//! The objects of a repository, read from its `objects` folder only: loose objects and packs, through
//! the confined root. Nothing else an object store can carry is used: no alternates (they would lead
//! out of the folder), no commit-graph, multi-pack index or bitmaps (git trusts them blindly), no
//! replace refs or grafts. Packs are read with positioned reads, never mapped: a file the agent
//! truncates while it is read is an error, not a crash. Every object, delta and delta chain is bounded,
//! and so are the pack indexes a store holds, together, and how often it looks for new ones.

use super::oid::Oid;
use crate::direct::root::{io_failure, too_large, Root};
use crate::error::{BridgeError, BridgeResult};
use crate::said::Said;
use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

/// The most packs a store is read from.
const PACKS_MAX: usize = 256;
/// The largest pack index read, in bytes.
const PACK_INDEX_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The most bytes of pack indexes one store holds, all its packs together.
const PACK_INDEXES_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// The least time between two looks for packs written since the store last looked.
const RESCAN_INTERVAL: Duration = Duration::from_secs(1);
/// The largest loose object read, compressed.
const LOOSE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The longest chain of deltas followed to build one object (git writes at most 50).
const DELTA_DEPTH_MAX: usize = 64;
/// A chain inflates at most this many times the object's limit, its base and deltas together, across
/// the bases it looks up by id: each delta is held until the base is reached. git keeps a delta only
/// when it is under half of what it makes, and most are a small part of it.
const DELTA_CHAIN_LIMITS: usize = 4;
/// The most bytes of objects kept decoded for the deltas that are built on them.
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// Delta bases larger than this are not kept.
const CACHE_ENTRY_MAX_BYTES: usize = 4 * 1024 * 1024;
/// The most objects kept decoded, whatever their size: an empty object costs memory too.
const CACHE_MAX_ENTRIES: usize = 16 * 1024;
/// What one kept object costs besides its bytes, counted against [`CACHE_MAX_BYTES`]: its key, its
/// handle, the header of its allocation and its slot in the map, rounded up.
const CACHE_ENTRY_OVERHEAD: usize = 128;
/// The header of a loose object is at most this long: its kind, a space, its size and a NUL.
const LOOSE_HEADER_MAX: usize = 32;
/// The header of a pack entry is at most this long: a type and size, then a base.
const ENTRY_HEADER_MAX: usize = 32;

/// What an object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    fn of_pack_type(value: u8) -> Option<Self> {
        match value {
            1 => Some(Kind::Commit),
            2 => Some(Kind::Tree),
            3 => Some(Kind::Blob),
            4 => Some(Kind::Tag),
            _ => None,
        }
    }

    fn of_name(name: &[u8]) -> Option<Self> {
        match name {
            b"commit" => Some(Kind::Commit),
            b"tree" => Some(Kind::Tree),
            b"blob" => Some(Kind::Blob),
            b"tag" => Some(Kind::Tag),
            _ => None,
        }
    }
}

/// One object, whole.
#[derive(Debug, Clone)]
pub struct Object {
    pub kind: Kind,
    pub data: Arc<Vec<u8>>,
}

/// What a store may hold of a repository the agent writes, and how often it looks at it again.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The most bytes of pack indexes held, all packs together.
    pub(crate) indexes_max_bytes: u64,
    /// The least time between two looks for new packs.
    pub(crate) rescan_interval: Duration,
}

const LIMITS: Limits = Limits {
    indexes_max_bytes: PACK_INDEXES_MAX_BYTES,
    rescan_interval: RESCAN_INTERVAL,
};

/// The objects folder of one repository.
pub struct Store {
    objects: Root,
    limits: Limits,
    packs: RwLock<Vec<Arc<Pack>>>,
    /// When the store last looked for new packs; nothing before its first look after opening.
    last_rescan: Mutex<Option<Instant>>,
    cache: Mutex<Cache>,
    /// The id the next pack read is given.
    next_pack: AtomicU64,
    /// The bytes of pack indexes in memory, of every pack still held, the retired ones a reader holds
    /// included.
    indexes: Arc<IndexBudget>,
}

impl Store {
    /// The store in `objects`, its packs' indexes read now.
    pub fn open(objects: Root) -> BridgeResult<Self> {
        Self::open_with(objects, LIMITS)
    }

    /// The same, held to `limits`.
    pub(crate) fn open_with(objects: Root, limits: Limits) -> BridgeResult<Self> {
        let next_pack = AtomicU64::new(0);
        let indexes = Arc::new(IndexBudget::new(limits.indexes_max_bytes));
        let stems = pack_stems(&objects)?;
        let packs = read_packs(&objects, &stems, &[], &next_pack, &indexes)?;
        Ok(Self {
            objects,
            limits,
            packs: RwLock::new(packs),
            last_rescan: Mutex::default(),
            cache: Mutex::default(),
            next_pack,
            indexes,
        })
    }

    /// The object `oid`, refused over `limit` bytes; nothing when the store does not hold it. Before an
    /// object is said to be missing, packs written since the last look are looked for, at most once per
    /// interval: the agent can name missing objects as often as it likes.
    pub fn read(&self, oid: Oid, limit: usize) -> BridgeResult<Option<Object>> {
        if let Some(found) = self.find(oid, limit, &mut Chain::new(limit))? {
            return Ok(Some(found));
        }
        self.rescan()?;
        self.find(oid, limit, &mut Chain::new(limit))
    }

    /// Lets go of the packs that are gone and forgets their objects, then reads the indexes of packs
    /// that appeared since the last look, keeping those already read. Every index still in memory counts
    /// against the store's cap, a retired one a reader still holds included. A read that waits here finds
    /// the packs the look found.
    fn rescan(&self) -> BridgeResult<()> {
        let mut last = self
            .last_rescan
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < self.limits.rescan_interval) {
            return Ok(());
        }
        *last = Some(Instant::now());
        let stems = pack_stems(&self.objects)?;
        let kept = {
            let mut packs = self.packs.write().unwrap_or_else(PoisonError::into_inner);
            packs.retain(|pack| stems.contains(&pack.stem));
            packs.clone()
        };
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keep_only(&kept);
        let packs = read_packs(&self.objects, &stems, &kept, &self.next_pack, &self.indexes)?;
        *self.packs.write().unwrap_or_else(PoisonError::into_inner) = packs;
        Ok(())
    }

    fn find(&self, oid: Oid, limit: usize, chain: &mut Chain) -> BridgeResult<Option<Object>> {
        let packs = self
            .packs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for pack in &packs {
            if let Some(offset) = pack.index.offset_of(&oid) {
                return self.object_in_pack(pack, offset, limit, chain).map(Some);
            }
        }
        self.loose(oid, limit, chain)
    }

    /// The loose object `oid`, its size charged to `chain` before its body is inflated: a loose object
    /// can be the base of a chain of packed deltas, and is then part of what the chain inflates.
    fn loose(&self, oid: Oid, limit: usize, chain: &mut Chain) -> BridgeResult<Option<Object>> {
        let hex = oid.hex();
        let path = format!("{}/{}", &hex[..2], &hex[2..]);
        let compressed = match self.objects.read(&path, LOOSE_MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.code == crate::error::ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let not_zlib = || corrupt("a loose object is not a zlib stream");
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut data = Vec::new();
        (&mut decoder)
            .take(LOOSE_HEADER_MAX as u64)
            .read_to_end(&mut data)
            .map_err(|_| not_zlib())?;
        let (kind, size, body_at) = loose_header(&data)?;
        if size > limit {
            return Err(too_large());
        }
        chain.charge(size)?;
        let mut body = data.split_off(body_at);
        let rest = size.saturating_sub(body.len());
        body.reserve(rest);
        decoder
            .take(rest as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|_| not_zlib())?;
        if body.len() != size {
            return Err(corrupt("a loose object is not as long as it says"));
        }
        Ok(Some(Object {
            kind,
            data: Arc::new(body),
        }))
    }

    /// The object at `offset` of `pack`, its deltas applied. What it inflates is charged to `chain`,
    /// which a base looked up by id goes on charging.
    fn object_in_pack(
        &self,
        pack: &Arc<Pack>,
        offset: u64,
        limit: usize,
        chain: &mut Chain,
    ) -> BridgeResult<Object> {
        let mut deltas: Vec<Vec<u8>> = Vec::new();
        let mut at = offset;
        let base = loop {
            if let Some(cached) = self.cached(pack, at) {
                break cached;
            }
            let entry = pack.entry(at)?;
            chain.charge(entry.size)?;
            match entry.base {
                Base::None(kind) => {
                    let data = pack.inflate(entry.data_at, entry.size, limit)?;
                    let object = Object {
                        kind,
                        data: Arc::new(data),
                    };
                    self.keep(pack, at, &object);
                    break object;
                }
                Base::Offset(base_at) => {
                    chain.deeper()?;
                    deltas.push(pack.inflate(entry.data_at, entry.size, limit)?);
                    at = base_at;
                }
                Base::Id(base_id) => {
                    chain.deeper()?;
                    deltas.push(pack.inflate(entry.data_at, entry.size, limit)?);
                    break self
                        .find(base_id, limit, chain)?
                        .ok_or_else(|| corrupt("the base of a delta is missing"))?;
                }
            }
        };
        let mut object = base;
        for delta in deltas.iter().rev() {
            object = Object {
                kind: object.kind,
                data: Arc::new(apply_delta(&object.data, delta, limit)?),
            };
        }
        self.keep(pack, offset, &object);
        Ok(object)
    }

    fn cached(&self, pack: &Arc<Pack>, offset: u64) -> Option<Object> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.objects.get(&(pack.id, offset)).cloned()
    }

    fn keep(&self, pack: &Arc<Pack>, offset: u64, object: &Object) {
        if object.data.len() > CACHE_ENTRY_MAX_BYTES {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.keep((pack.id, offset), object);
    }
}

/// What building one object may still spend: how many more deltas it follows and how many more bytes
/// it inflates, across the bases it looks up by id.
struct Chain {
    depth: usize,
    bytes_left: usize,
}

impl Chain {
    fn new(limit: usize) -> Self {
        Self {
            depth: 0,
            bytes_left: limit.saturating_mul(DELTA_CHAIN_LIMITS),
        }
    }

    /// Counts `bytes` about to be inflated; past the chain's share, the object is too large to build.
    fn charge(&mut self, bytes: usize) -> BridgeResult<()> {
        self.bytes_left = self.bytes_left.checked_sub(bytes).ok_or_else(too_large)?;
        Ok(())
    }

    /// Counts one more delta followed.
    fn deeper(&mut self) -> BridgeResult<()> {
        self.depth += 1;
        match self.depth > DELTA_DEPTH_MAX {
            true => Err(corrupt("a chain of deltas is too long")),
            false => Ok(()),
        }
    }
}

/// Decoded objects of packs, by pack id and offset, dropped whole when full. Each object is counted at
/// its size and [`CACHE_ENTRY_OVERHEAD`], and there are at most [`CACHE_MAX_ENTRIES`] of them.
#[derive(Default)]
struct Cache {
    objects: HashMap<(u64, u64), Object>,
    bytes: usize,
}

impl Cache {
    fn keep(&mut self, key: (u64, u64), object: &Object) {
        if self.objects.contains_key(&key) {
            return;
        }
        let cost = object.data.len() + CACHE_ENTRY_OVERHEAD;
        if self.objects.len() >= CACHE_MAX_ENTRIES || self.bytes + cost > CACHE_MAX_BYTES {
            self.objects.clear();
            self.bytes = 0;
        }
        self.bytes += cost;
        self.objects.insert(key, object.clone());
    }

    /// Forgets the objects of every pack that is not one of `packs`.
    fn keep_only(&mut self, packs: &[Arc<Pack>]) {
        self.objects
            .retain(|(pack, _), _| packs.iter().any(|live| live.id == *pack));
        self.bytes = self
            .objects
            .values()
            .map(|object| object.data.len() + CACHE_ENTRY_OVERHEAD)
            .sum();
    }
}

/// One pack: its index in memory and its data file, open.
struct Pack {
    /// Its id in the store, never given to another pack: what the cache knows its objects by.
    id: u64,
    /// The name of its files without the extension (`pack-<hash>`).
    stem: String,
    index: PackIndex,
    /// The index's bytes, counted against the store's cap for as long as the pack is held.
    _index_charge: Charge,
    data: File,
    size: u64,
}

/// Where an entry of a pack takes its content from.
enum Base {
    None(Kind),
    Offset(u64),
    Id(Oid),
}

struct Entry {
    base: Base,
    /// The size of what the entry inflates to: the object, or the delta.
    size: usize,
    data_at: u64,
}

impl Pack {
    fn entry(&self, offset: u64) -> BridgeResult<Entry> {
        let mut header = [0u8; ENTRY_HEADER_MAX];
        let room = usize::try_from(self.size.saturating_sub(offset))
            .unwrap_or(usize::MAX)
            .min(ENTRY_HEADER_MAX);
        let header = &mut header[..room];
        self.data
            .read_exact_at(header, offset)
            .map_err(io_failure)?;
        let mut at = 0;
        let mut byte = *header
            .first()
            .ok_or_else(|| corrupt("an entry is past the pack's end"))?;
        let kind = (byte >> 4) & 0x7;
        let mut size = u64::from(byte & 0x0f);
        let mut shift = 4;
        while byte & 0x80 != 0 {
            at += 1;
            byte = *header
                .get(at)
                .ok_or_else(|| corrupt("an entry's size runs past its header"))?;
            if shift > 56 {
                return Err(corrupt("an entry's size is too large"));
            }
            size |= u64::from(byte & 0x7f) << shift;
            shift += 7;
        }
        at += 1;
        let base = match kind {
            6 => {
                let mut byte = *header
                    .get(at)
                    .ok_or_else(|| corrupt("a delta has no base"))?;
                let mut distance = u64::from(byte & 0x7f);
                while byte & 0x80 != 0 {
                    at += 1;
                    byte = *header
                        .get(at)
                        .ok_or_else(|| corrupt("a delta's base runs past its header"))?;
                    distance = distance
                        .checked_add(1)
                        .and_then(|d| d.checked_mul(128))
                        .ok_or_else(|| corrupt("a delta's base is too far"))?
                        | u64::from(byte & 0x7f);
                }
                at += 1;
                let base = offset.checked_sub(distance).filter(|base| *base < offset);
                Base::Offset(base.ok_or_else(|| corrupt("a delta's base is not before it"))?)
            }
            7 => {
                let id = header.get(at..at + 20).and_then(Oid::from_bytes);
                at += 20;
                Base::Id(id.ok_or_else(|| corrupt("a delta's base runs past its header"))?)
            }
            other => Base::None(
                Kind::of_pack_type(other).ok_or_else(|| corrupt("an entry of an unknown kind"))?,
            ),
        };
        Ok(Entry {
            base,
            size: usize::try_from(size).map_err(|_| too_large())?,
            data_at: offset + at as u64,
        })
    }

    /// Inflates the zlib stream at `offset`, which must give exactly `size` bytes.
    fn inflate(&self, offset: u64, size: usize, limit: usize) -> BridgeResult<Vec<u8>> {
        if size > limit {
            return Err(too_large());
        }
        let reader = PackReader {
            file: &self.data,
            at: offset,
            end: self.size,
        };
        let mut data = Vec::with_capacity(size);
        ZlibDecoder::new(BufReader::with_capacity(64 * 1024, reader))
            .take(size as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| corrupt("an entry of a pack is not a zlib stream"))?;
        match data.len() == size {
            true => Ok(data),
            false => Err(corrupt("an entry of a pack is not as long as it says")),
        }
    }
}

/// Reads a pack from an offset with positioned reads.
struct PackReader<'a> {
    file: &'a File,
    at: u64,
    end: u64,
}

impl Read for PackReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let left = usize::try_from(self.end.saturating_sub(self.at)).unwrap_or(usize::MAX);
        let room = buffer.len().min(left);
        let read = self.file.read_at(&mut buffer[..room], self.at)?;
        self.at += read as u64;
        Ok(read)
    }
}

/// The ids of a pack, sorted, and where each entry starts: the index file as read, never copied, with
/// where its tables are in it.
struct PackIndex {
    fanout: Vec<u32>,
    bytes: Vec<u8>,
    count: usize,
    ids_at: usize,
    offsets_at: usize,
    large: std::ops::Range<usize>,
}

impl PackIndex {
    /// A version 2 index.
    fn parse(bytes: Vec<u8>) -> BridgeResult<Self> {
        const FANOUT_AT: usize = 8;
        let malformed = || corrupt("a pack index is malformed");
        if bytes.get(..8) != Some(&[0xff, b't', b'O', b'c', 0, 0, 0, 2][..]) {
            return Err(corrupt("a pack index is not of version 2"));
        }
        let fanout: Vec<u32> = bytes
            .get(FANOUT_AT..FANOUT_AT + 1024)
            .ok_or_else(malformed)?
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().expect("four bytes")))
            .collect();
        if fanout.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err(malformed());
        }
        let count = *fanout.last().expect("256 entries") as usize;
        let ids_at = FANOUT_AT + 1024;
        let offsets_at = ids_at + count * 24;
        let large_at = offsets_at + count * 4;
        if bytes.len() < large_at {
            return Err(malformed());
        }
        let large = large_at..bytes.len().saturating_sub(40).max(large_at);
        Ok(Self {
            fanout,
            bytes,
            count,
            ids_at,
            offsets_at,
            large,
        })
    }

    fn offset_of(&self, oid: &Oid) -> Option<u64> {
        let first = oid.0[0] as usize;
        let start = if first == 0 {
            0
        } else {
            self.fanout[first - 1] as usize
        };
        let end = self.fanout[first] as usize;
        let (mut low, mut high) = (start, end);
        while low < high {
            let middle = (low + high) / 2;
            let at = self.ids_at + middle * 20;
            match self.bytes[at..at + 20].cmp(&oid.0[..]) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return self.offset_at(middle),
            }
        }
        None
    }

    fn offset_at(&self, position: usize) -> Option<u64> {
        if position >= self.count {
            return None;
        }
        let at = self.offsets_at + position * 4;
        let word = u32::from_be_bytes(self.bytes.get(at..at + 4)?.try_into().ok()?);
        if word & 0x8000_0000 == 0 {
            return Some(u64::from(word));
        }
        let at = (word & 0x7fff_ffff) as usize * 8;
        let large = self.bytes.get(self.large.clone())?;
        Some(u64::from_be_bytes(large.get(at..at + 8)?.try_into().ok()?))
    }
}

/// The bytes of pack indexes a store may hold in memory, and how many it holds now.
struct IndexBudget {
    max_bytes: u64,
    held: AtomicU64,
}

impl IndexBudget {
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            held: AtomicU64::new(0),
        }
    }

    /// Counts `bytes` about to be read, until the charge is dropped; past the cap, nothing is counted
    /// and the read is refused.
    fn charge(self: &Arc<Self>, bytes: u64) -> BridgeResult<Charge> {
        let mut held = self.held.load(Ordering::Acquire);
        loop {
            let total = held
                .checked_add(bytes)
                .filter(|total| *total <= self.max_bytes)
                .ok_or_else(too_large)?;
            match self
                .held
                .compare_exchange_weak(held, total, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(now) => held = now,
            }
        }
        Ok(Charge {
            budget: self.clone(),
            bytes,
        })
    }
}

/// Bytes counted against an [`IndexBudget`], given back when dropped.
struct Charge {
    budget: Arc<IndexBudget>,
    bytes: u64,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.held.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// The packs named `stems` whose index and data can both be opened, those in `kept` kept as they are (a
/// pack's name is the hash of its content), the others read now within what `budget` has left: past it
/// the store is refused, as a pack left out would make its objects look missing.
fn read_packs(
    objects: &Root,
    stems: &[String],
    kept: &[Arc<Pack>],
    ids: &AtomicU64,
    budget: &Arc<IndexBudget>,
) -> BridgeResult<Vec<Arc<Pack>>> {
    let mut packs = Vec::with_capacity(stems.len());
    for stem in stems {
        let pack = match kept.iter().find(|pack| pack.stem == *stem) {
            Some(pack) => Some(pack.clone()),
            None => open_pack(objects, stem.clone(), ids, budget)?,
        };
        packs.extend(pack);
    }
    Ok(packs)
}

/// The names of the packs in `objects/pack`, without extension, the first `PACKS_MAX` in order.
fn pack_stems(objects: &Root) -> BridgeResult<Vec<String>> {
    let folder = match objects.dir("pack") {
        Ok(folder) => folder,
        Err(error) if error.code == crate::error::ErrorCode::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut stems: Vec<String> = folder
        .list()?
        .entries
        .into_iter()
        .filter_map(|(name, _)| name.strip_suffix(".idx").map(str::to_string))
        .filter(|stem| stem.starts_with("pack-"))
        .collect();
    stems.sort();
    stems.truncate(PACKS_MAX);
    Ok(stems)
}

/// The pack `stem`, its index read if `budget` has room for it, the index held as read; nothing when
/// either file cannot be opened or the data is not a pack.
fn open_pack(
    objects: &Root,
    stem: String,
    ids: &AtomicU64,
    budget: &Arc<IndexBudget>,
) -> BridgeResult<Option<Arc<Pack>>> {
    let Ok((index_file, index_stat)) = objects.open_file(&format!("pack/{stem}.idx")) else {
        return Ok(None);
    };
    if index_stat.size > PACK_INDEX_MAX_BYTES {
        return Err(too_large());
    }
    let Ok((data, stat)) = objects.open_file(&format!("pack/{stem}.pack")) else {
        return Ok(None);
    };
    let mut header = [0u8; 8];
    let is_pack = data.read_exact_at(&mut header, 0).is_ok()
        && &header[..4] == b"PACK"
        && matches!(header[7], 2 | 3);
    if !is_pack {
        return Ok(None);
    }
    let charge = budget.charge(index_stat.size)?;
    let index = read_whole(index_file, index_stat.size)?;
    Ok(Some(Arc::new(Pack {
        id: ids.fetch_add(1, Ordering::Relaxed),
        stem,
        index: PackIndex::parse(index)?,
        _index_charge: charge,
        data,
        size: stat.size,
    })))
}

/// The `size` bytes of `file`; a file that grew or shrank since its size was taken is an error. The
/// buffer is exactly `size` bytes and never grows: the end is checked with one byte apart from it.
fn read_whole(mut file: File, size: u64) -> BridgeResult<Vec<u8>> {
    let changed = || corrupt("a pack index changed while it was read");
    let mut bytes = vec![0u8; usize::try_from(size).map_err(|_| too_large())?];
    file.read_exact(&mut bytes)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => changed(),
            _ => io_failure(error),
        })?;
    let mut after = [0u8; 1];
    match file.read(&mut after).map_err(io_failure)? {
        0 => Ok(bytes),
        _ => Err(changed()),
    }
}

/// The object a delta makes of `base`.
fn apply_delta(base: &[u8], delta: &[u8], limit: usize) -> BridgeResult<Vec<u8>> {
    let malformed = || corrupt("a delta is malformed");
    let (source, mut at) = delta_size(delta, 0).ok_or_else(malformed)?;
    let (target, next) = delta_size(delta, at).ok_or_else(malformed)?;
    at = next;
    if source != base.len() {
        return Err(malformed());
    }
    if target > limit {
        return Err(too_large());
    }
    let mut out = Vec::with_capacity(target);
    while at < delta.len() {
        let op = delta[at];
        at += 1;
        if op & 0x80 != 0 {
            let mut fields = [0usize; 7];
            for (bit, field) in fields.iter_mut().enumerate() {
                if op & (1 << bit) != 0 {
                    *field = *delta.get(at).ok_or_else(malformed)? as usize;
                    at += 1;
                }
            }
            let offset = fields[0] | fields[1] << 8 | fields[2] << 16 | fields[3] << 24;
            let size = match fields[4] | fields[5] << 8 | fields[6] << 16 {
                0 => 0x10000,
                size => size,
            };
            let copied = base
                .get(offset..offset.checked_add(size).ok_or_else(malformed)?)
                .ok_or_else(malformed)?;
            out.extend_from_slice(copied);
        } else if op != 0 {
            let inserted = delta.get(at..at + op as usize).ok_or_else(malformed)?;
            out.extend_from_slice(inserted);
            at += op as usize;
        } else {
            return Err(malformed());
        }
        if out.len() > target {
            return Err(malformed());
        }
    }
    match out.len() == target {
        true => Ok(out),
        false => Err(malformed()),
    }
}

fn delta_size(delta: &[u8], mut at: usize) -> Option<(usize, usize)> {
    let mut size = 0usize;
    let mut shift = 0;
    loop {
        let byte = *delta.get(at)?;
        at += 1;
        if shift > 56 {
            return None;
        }
        size |= ((byte & 0x7f) as usize) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((size, at));
        }
    }
}

/// The kind and size a loose object's header states, and where its body starts in `data`, which holds
/// at least the header.
fn loose_header(data: &[u8]) -> BridgeResult<(Kind, usize, usize)> {
    let nul = data
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| corrupt("a loose object has no header"))?;
    let header = &data[..nul];
    let space = header
        .iter()
        .position(|b| *b == b' ')
        .ok_or_else(|| corrupt("a loose object has no size"))?;
    let kind = Kind::of_name(&header[..space])
        .ok_or_else(|| corrupt("a loose object of an unknown kind"))?;
    let size: usize = std::str::from_utf8(&header[space + 1..])
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| corrupt("a loose object's size is not a number"))?;
    Ok((kind, size, nul + 1))
}

/// A repository that cannot be read as git writes it.
pub(crate) fn corrupt(why: &str) -> BridgeError {
    BridgeError::failed(Said::new("direct.repository")).with_details(Some(why.to_string()))
}

#[cfg(test)]
#[path = "objects_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn apply_delta_for_test(
    base: &[u8],
    delta: &[u8],
    limit: usize,
) -> BridgeResult<Vec<u8>> {
    apply_delta(base, delta, limit)
}
