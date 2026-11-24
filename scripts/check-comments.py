import re
import sys
from dataclasses import dataclass
from pathlib import Path


TOKEN = re.compile(
    r'(?P<raw>(?:br|cr|r)(?P<hashes>#{0,255})")'
    r'|"(?:\\[\s\S]|[^"\\])*"'
    r"|'(?:\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|[^\n])|[^'\\\n])'"
    r'|(?P<line>//[^\n]*)|(?P<block>/\*)'
)
BLOCK_MARK = re.compile(r'/\*|\*/')
SEPARATOR = re.compile(r'^(?:[-=*_#]{4,}(?=\s|$)|[\u2500-\u257f]{3,})')
STEP = re.compile(r'^step\s+\d+\b', re.IGNORECASE)
EMOJI = re.compile(
    '[\U0001f000-\U0001faff\u2600-\u26ff\u2700-\u27bf\u231a-\u231b\u23e9-\u23f3'
    '\u23f8-\u23fa\u2b05-\u2b07\u2b1b-\u2b1c\u2b50\u2b55\u20e3\ufe0f]'
)


@dataclass
class Comment:
    line: int
    text: str
    marker: str
    start: int
    end: int
    standalone: bool


def block_end(source, start):
    depth = 1
    for mark in BLOCK_MARK.finditer(source, start):
        depth += 1 if mark[0] == '/*' else -1
        if depth == 0:
            return mark.end()
    return len(source)


def comments(source):
    cursor = 0
    previous = 0
    line = 1
    while match := TOKEN.search(source, cursor):
        start, end = match.span()
        cursor = end
        if match['raw']:
            close = '"' + match['hashes']
            found = source.find(close, end)
            cursor = len(source) if found < 0 else found + len(close)
            continue
        if not (match['line'] or match['block']):
            continue
        line += source.count('\n', previous, start)
        previous = start
        standalone = not source[source.rfind('\n', 0, start) + 1:start].strip()
        if match['line']:
            marker = source[start:start + 3] if source[start + 2:end].startswith(('!', '/')) else '//'
            yield Comment(line, source[start + len(marker):end], marker, start, end, standalone)
        else:
            cursor = block_end(source, end)
            marker = '/*!' if source[end:end + 1] == '!' else '/*'
            text = source[start + len(marker):cursor - 2]
            yield Comment(line, text, marker, start, cursor, standalone)


def groups(source):
    group = []
    for comment in comments(source):
        if group and not joins(group[-1], comment):
            yield group
            group = []
        group.append(comment)
    if group:
        yield group


def joins(previous, current):
    return (
        previous.marker == current.marker
        and previous.marker.startswith('//')
        and previous.standalone and current.standalone
        and previous.line + 1 == current.line
    )


def problems(group):
    first = group[0]
    notes = []
    height = 0
    for comment in group:
        lines = comment.text.split('\n')
        height += len(lines)
        for line, words in enumerate(lines, comment.line):
            for part in BLOCK_MARK.split(words):
                body = re.sub(r'^\s*\*\s+', '', part).strip()
                notes.append((line, body))
    exempt = first.marker in ('//!', '/*!') or any(
        body.startswith(('SAFETY:', '# Safety')) for _, body in notes
    )
    if height > 8 and not exempt:
        yield first.line, 'comment exceeds 8 lines'
    for line, body in notes:
        if SEPARATOR.search(body):
            yield line, 'decorative separator'
        if STEP.search(body):
            yield line, 'numbered step'
        if EMOJI.search(body):
            yield line, 'emoji in comment'


def main(arguments):
    paths = []
    for argument in arguments:
        path = Path(argument)
        paths.extend(path.rglob('*.rs') if path.is_dir() else [path])
    failed = False
    for path in sorted(set(paths)):
        source = path.read_text(encoding='utf-8')
        for group in groups(source):
            for line, message in problems(group):
                print(f'{path}:{line}: {message}', file=sys.stderr)
                failed = True
    return int(failed)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
