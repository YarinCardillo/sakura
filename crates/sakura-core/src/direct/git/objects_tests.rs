//! The object store against repositories written by hand: git never writes the hostile shapes an agent
//! can (pack indexes that add up to more than the app should hold, chains of deltas each within the
//! limit but too large together), so the packs and their indexes are built byte by byte here.

use super::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// A pack and its version 2 index, with ids of the test's choosing. The reader checks neither the
/// checksums nor that an id is the hash of its object, so they are left at zero.
struct HandPack {
    data: Vec<u8>,
    entries: Vec<([u8; 20], u64)>,
}

impl HandPack {
    fn new() -> Self {
        Self {
            data: b"PACK\0\0\0\x02\0\0\0\0".to_vec(),
            entries: Vec::new(),
        }
    }

    fn blob(&mut self, id: [u8; 20], body: &[u8]) -> u64 {
        self.push(id, 3, &[], body)
    }

    fn offset_delta(&mut self, id: [u8; 20], base: u64, delta: &[u8]) -> u64 {
        let mut distance = self.data.len() as u64 - base;
        let mut encoded = vec![(distance & 0x7f) as u8];
        distance >>= 7;
        while distance != 0 {
            distance -= 1;
            encoded.push(0x80 | (distance & 0x7f) as u8);
            distance >>= 7;
        }
        encoded.reverse();
        self.push(id, 6, &encoded, delta)
    }

    fn id_delta(&mut self, id: [u8; 20], base: [u8; 20], delta: &[u8]) -> u64 {
        self.push(id, 7, &base, delta)
    }

    fn push(&mut self, id: [u8; 20], kind: u8, base: &[u8], body: &[u8]) -> u64 {
        let offset = self.data.len() as u64;
        let mut size = body.len();
        let mut byte = (kind << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size != 0 {
            self.data.push(byte | 0x80);
            byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        self.data.push(byte);
        self.data.extend_from_slice(base);
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(body).unwrap();
        self.data.extend(encoder.finish().unwrap());
        self.entries.push((id, offset));
        offset
    }

    /// Writes `pack-<stem>.pack` and `pack-<stem>.idx` into `objects/pack`.
    fn write(&self, objects: &Path, stem: &str) {
        let folder = objects.join("pack");
        fs::create_dir_all(&folder).unwrap();
        let mut pack = self.data.clone();
        pack.extend([0u8; 20]);
        fs::write(folder.join(format!("pack-{stem}.pack")), pack).unwrap();
        fs::write(folder.join(format!("pack-{stem}.idx")), self.index()).unwrap();
    }

    fn index(&self) -> Vec<u8> {
        let mut entries = self.entries.clone();
        entries.sort();
        let mut index = vec![0xff, b't', b'O', b'c', 0, 0, 0, 2];
        for first in 0..=255u8 {
            let count = entries.iter().filter(|(id, _)| id[0] <= first).count() as u32;
            index.extend(count.to_be_bytes());
        }
        entries.iter().for_each(|(id, _)| index.extend(id));
        entries.iter().for_each(|_| index.extend([0u8; 4]));
        entries
            .iter()
            .for_each(|(_, offset)| index.extend((*offset as u32).to_be_bytes()));
        index.extend([0u8; 40]);
        index
    }
}

/// A delta that makes `target` out of a base of `source` bytes, inserting every byte.
fn inserting_delta(source: usize, target: &[u8]) -> Vec<u8> {
    let mut delta = Vec::new();
    for mut size in [source, target.len()] {
        while size >= 0x80 {
            delta.push((size & 0x7f) as u8 | 0x80);
            size >>= 7;
        }
        delta.push(size as u8);
    }
    for chunk in target.chunks(0x7f) {
        delta.push(chunk.len() as u8);
        delta.extend_from_slice(chunk);
    }
    delta
}

/// A pack of `count` blobs of `size` bytes each, so its index holds `count` ids.
fn pack_of(first: u8, count: usize, size: usize) -> HandPack {
    let mut pack = HandPack::new();
    for n in 0..count {
        let mut id = [first; 20];
        id[1..9].copy_from_slice(&(n as u64).to_be_bytes());
        pack.blob(id, &vec![b'x'; size]);
    }
    pack
}

fn objects_folder() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let objects = fs::canonicalize(temp.path()).unwrap().join("objects");
    fs::create_dir_all(&objects).unwrap();
    (temp, objects)
}

fn open(objects: &Path, limits: Limits) -> BridgeResult<Store> {
    Store::open_with(Root::metadata(objects).unwrap(), limits)
}

const INDEXES_OF_TESTS: Limits = Limits {
    indexes_max_bytes: 64 * 1024,
    rescan_interval: Duration::ZERO,
};

#[test]
fn packs_whose_indexes_add_up_past_the_store_cap_are_refused() {
    let (_temp, objects) = objects_folder();
    for (n, stem) in ["a", "b", "c"].into_iter().enumerate() {
        pack_of(n as u8 + 1, 1000, 1).write(&objects, stem);
    }
    let error = open(&objects, INDEXES_OF_TESTS).err().unwrap();
    assert_eq!(error.said.key.0, "direct.too_large");
}

#[test]
fn a_rescan_holds_the_packs_already_read_and_the_new_ones_to_the_same_cap() {
    let (_temp, objects) = objects_folder();
    pack_of(1, 1000, 1).write(&objects, "a");
    pack_of(2, 1000, 1).write(&objects, "b");
    let store = open(&objects, INDEXES_OF_TESTS).unwrap();
    pack_of(3, 1000, 1).write(&objects, "c");
    let error = store.read(Oid([9; 20]), 1024).unwrap_err();
    assert_eq!(error.said.key.0, "direct.too_large");
}

/// The id of the `n`th object of a chain.
fn link(n: u8) -> [u8; 20] {
    [n + 1; 20]
}

const LINK_BYTES: usize = 16 * 1024;
const LINKS: u8 = 40;

/// A blob and a chain of `LINKS` deltas on it, each rewriting all of its `LINK_BYTES` bytes. The middle
/// delta names its base by id, so the chain is read in two halves, the second through a lookup.
fn hostile_chain(objects: &Path) {
    let mut pack = HandPack::new();
    let mut at = pack.blob(link(0), &vec![b'a'; LINK_BYTES]);
    for n in 1..=LINKS {
        let delta = inserting_delta(LINK_BYTES, &vec![b'a' + n % 26; LINK_BYTES]);
        at = match n == LINKS / 2 {
            true => pack.id_delta(link(n), link(n - 1), &delta),
            false => pack.offset_delta(link(n), at, &delta),
        };
    }
    pack.write(objects, "chain");
}

#[test]
fn a_chain_of_deltas_each_within_the_limit_but_too_large_together_is_refused() {
    let (_temp, objects) = objects_folder();
    hostile_chain(&objects);
    let last = Oid(link(LINKS));
    let roomy = open(&objects, LIMITS).unwrap().read(last, 256 * 1024);
    let built = roomy.unwrap().unwrap();
    assert_eq!(*built.data, vec![b'a' + LINKS % 26; LINK_BYTES]);
    let half = LINK_BYTES * usize::from(LINKS) / 2;
    let limit = half / DELTA_CHAIN_LIMITS + LINK_BYTES;
    let error = open(&objects, LIMITS)
        .unwrap()
        .read(last, limit)
        .unwrap_err();
    assert_eq!(error.said.key.0, "direct.too_large");
}

#[test]
fn a_missing_object_looks_for_new_packs_at_most_once_per_interval() {
    let wanted = Oid([2; 20]);
    for (interval, is_found) in [(Duration::from_secs(3600), false), (Duration::ZERO, true)] {
        let (_temp, objects) = objects_folder();
        pack_of(1, 1, 4).write(&objects, "a");
        let limits = Limits {
            indexes_max_bytes: PACK_INDEXES_MAX_BYTES,
            rescan_interval: interval,
        };
        let store = open(&objects, limits).unwrap();
        assert!(store.read(wanted, 1024).unwrap().is_none());
        let mut pack = HandPack::new();
        pack.blob(wanted.0, b"late");
        pack.write(&objects, "b");
        let found = store.read(wanted, 1024).unwrap();
        assert_eq!(found.is_some(), is_found, "interval {interval:?}");
    }
}

/// Writes the loose blob `id` holding `body` into `objects`.
fn write_loose(objects: &Path, id: [u8; 20], body: &[u8]) {
    let hex = Oid(id).hex();
    let folder = objects.join(&hex[..2]);
    fs::create_dir_all(&folder).unwrap();
    let mut raw = format!("blob {}\0", body.len()).into_bytes();
    raw.extend_from_slice(body);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&raw).unwrap();
    fs::write(folder.join(&hex[2..]), encoder.finish().unwrap()).unwrap();
}

/// The review's mixed chain: four packed deltas, each just under the limit, the first naming by id a
/// loose base of the limit's size. Every piece fits the limit; together they inflate almost five times
/// it, past the chain's share.
#[test]
fn a_chain_of_packed_deltas_on_a_loose_base_is_capped_as_a_whole() {
    const LIMIT: usize = 16 * 1024;
    let made = LIMIT * 98 / 100;
    let (_temp, objects) = objects_folder();
    write_loose(&objects, link(0), &vec![b'a'; LIMIT]);
    let mut pack = HandPack::new();
    let first = inserting_delta(LIMIT, &vec![b'b'; made]);
    let mut at = pack.id_delta(link(1), link(0), &first);
    for n in 2..=4u8 {
        let delta = inserting_delta(made, &vec![b'a' + n; made]);
        assert!(delta.len() <= LIMIT, "each delta is within the limit");
        at = pack.offset_delta(link(n), at, &delta);
    }
    pack.write(&objects, "mixed");
    let last = Oid(link(4));
    let roomy = open(&objects, LIMITS).unwrap().read(last, 2 * LIMIT);
    assert_eq!(*roomy.unwrap().unwrap().data, vec![b'a' + 4; made]);
    let error = open(&objects, LIMITS)
        .unwrap()
        .read(last, LIMIT)
        .unwrap_err();
    assert_eq!(error.said.key.0, "direct.too_large");
}

/// The objects the cache holds and the bytes it counts for them.
fn cache_of(store: &Store) -> (usize, usize) {
    let cache = store.cache.lock().unwrap();
    (cache.objects.len(), cache.bytes)
}

/// The review's empty objects: each one read adds an entry to the cache and nothing to its bytes, so a
/// byte budget alone does not bound what the cache holds.
#[test]
fn the_cache_of_decoded_objects_is_bounded_by_entries_and_counts_their_overhead() {
    let (_temp, objects) = objects_folder();
    let count = CACHE_MAX_ENTRIES + 100;
    pack_of(1, count, 0).write(&objects, "empty");
    let store = open(&objects, LIMITS).unwrap();
    for n in 0..count {
        let mut id = [1u8; 20];
        id[1..9].copy_from_slice(&(n as u64).to_be_bytes());
        assert!(store.read(Oid(id), 1024).unwrap().is_some());
    }
    let (entries, bytes) = cache_of(&store);
    assert!(entries <= CACHE_MAX_ENTRIES, "{entries} entries");
    assert!(entries > 0);
    assert!(bytes >= entries * CACHE_ENTRY_OVERHEAD, "{bytes} bytes");
}

#[test]
fn the_cache_forgets_the_objects_of_packs_that_are_gone() {
    let (_temp, objects) = objects_folder();
    let mut first = HandPack::new();
    first.blob([1; 20], b"first");
    first.write(&objects, "a");
    let store = open(&objects, INDEXES_OF_TESTS).unwrap();
    assert!(store.read(Oid([1; 20]), 1024).unwrap().is_some());
    assert_eq!(cache_of(&store).0, 1);
    fs::remove_file(objects.join("pack/pack-a.idx")).unwrap();
    fs::remove_file(objects.join("pack/pack-a.pack")).unwrap();
    let mut second = HandPack::new();
    second.blob([2; 20], b"second");
    second.write(&objects, "b");
    assert!(store.read(Oid([9; 20]), 1024).unwrap().is_none());
    assert_eq!(
        cache_of(&store),
        (0, 0),
        "nothing of the retired pack is held"
    );
}

/// Replaces the packs `a` and `b` of `objects` with `c` and `d`, of other names and as large.
fn replace_packs(objects: &Path) {
    for stem in ["a", "b"] {
        fs::remove_file(objects.join(format!("pack/pack-{stem}.idx"))).unwrap();
        fs::remove_file(objects.join(format!("pack/pack-{stem}.pack"))).unwrap();
    }
    pack_of(3, 1000, 1).write(objects, "c");
    pack_of(4, 1000, 1).write(objects, "d");
}

/// The review's replacement: indexes that fill the cap give way to others of other names that fill it
/// again. Once the old ones are let go, the new ones are read.
#[test]
fn a_rescan_that_replaces_every_pack_lets_the_old_indexes_go_before_reading_the_new() {
    let (_temp, objects) = objects_folder();
    pack_of(1, 1000, 1).write(&objects, "a");
    pack_of(2, 1000, 1).write(&objects, "b");
    let store = open(&objects, INDEXES_OF_TESTS).unwrap();
    replace_packs(&objects);
    let mut moved = [3u8; 20];
    moved[1..9].copy_from_slice(&7u64.to_be_bytes());
    assert!(store.read(Oid(moved), 1024).unwrap().is_some());
}

/// While a reader still holds the old indexes they are still in memory, so the new ones would take the
/// store past its cap: the rescan is refused.
#[test]
fn a_rescan_counts_the_indexes_a_reader_still_holds() {
    let (_temp, objects) = objects_folder();
    pack_of(1, 1000, 1).write(&objects, "a");
    pack_of(2, 1000, 1).write(&objects, "b");
    let store = open(&objects, INDEXES_OF_TESTS).unwrap();
    let reader = store.packs.read().unwrap().clone();
    replace_packs(&objects);
    let error = store.read(Oid([9; 20]), 1024).unwrap_err();
    assert_eq!(error.said.key.0, "direct.too_large");
    drop(reader);
}

/// The largest single allocation the current thread requested while [`largest_allocation`] watched.
mod watched {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub(super) struct Watching;

    thread_local! {
        static IS_WATCHED: Cell<bool> = const { Cell::new(false) };
        static LARGEST: Cell<usize> = const { Cell::new(0) };
    }

    fn note(size: usize) {
        let is_watched = IS_WATCHED.try_with(Cell::get).unwrap_or(false);
        if is_watched {
            LARGEST
                .try_with(|largest| largest.set(largest.get().max(size)))
                .ok();
        }
    }

    unsafe impl GlobalAlloc for Watching {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note(layout.size());
            System.alloc(layout)
        }
        unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
            System.dealloc(at, layout)
        }
        unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            note(size);
            System.realloc(at, layout, size)
        }
    }

    /// Runs `work` and gives the largest allocation it made on this thread.
    pub(super) fn largest_allocation(work: impl FnOnce()) -> usize {
        LARGEST.with(|largest| largest.set(0));
        IS_WATCHED.with(|watched| watched.set(true));
        work();
        IS_WATCHED.with(|watched| watched.set(false));
        LARGEST.with(Cell::get)
    }
}

#[global_allocator]
static WATCHING: watched::Watching = watched::Watching;

/// The review's growing index: an index measured at one size that is longer when read is refused
/// without its buffer ever growing past the size it was charged for.
#[test]
fn an_index_that_grew_after_it_was_measured_is_refused_within_its_measured_size() {
    let (_temp, objects) = objects_folder();
    let path = objects.join("grown.idx");
    let measured: u64 = 256 * 1024;
    fs::write(&path, vec![7u8; 2 * measured as usize]).unwrap();
    let mut refused = None;
    let largest = watched::largest_allocation(|| {
        let file = File::open(&path).unwrap();
        refused = Some(read_whole(file, measured).unwrap_err());
    });
    assert!(refused.is_some());
    assert!(
        largest <= measured as usize + 64,
        "the read allocated {largest} bytes for an index of {measured}"
    );
}
