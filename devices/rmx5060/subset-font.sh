#!/bin/sh
# Make the UI typeface small enough to travel in a recovery: every character
# the pages' texts are written with, and the marks they are set among.
set -e
root=$(dirname "$0")/../..
python3 - "$root" > /tmp/skyward-glyphs.txt <<'PY'
import glob, sys
root = sys.argv[1]
chars = set()
for path in glob.glob(f'{root}/crates/skyward-ui/locales/*.toml'):
    chars.update(open(path, encoding='utf8').read())
chars.update(chr(c) for c in range(0x20, 0x7F))
for a, b in [(0x3000, 0x303F), (0xFF00, 0xFFEF), (0x2190, 0x21FF), (0x2000, 0x206F), (0x00A0, 0x00BF), (0x0400, 0x04FF), (0x1E00, 0x1EFF), (0x0100, 0x017F)]:
    chars.update(chr(c) for c in range(a, b))
chars = {c for c in chars if not (0xD800 <= ord(c) <= 0xDFFF) and c not in '\n\r\t'}
sys.stdout.write(''.join(sorted(chars)))
PY
python3 -m fontTools.subset "$root/resources/fonts/MiSansVF.ttf" \
    --text-file=/tmp/skyward-glyphs.txt \
    --output-file="$(dirname "$0")/MiSansVF-subset.ttf" \
    --layout-features='*' --glyph-names --symbol-cmap --legacy-cmap \
    --notdef-glyph --notdef-outline --recommended-glyphs \
    --name-IDs='*' --name-legacy --name-languages='*'
ls -l "$(dirname "$0")/MiSansVF-subset.ttf"
