#!/usr/bin/env python3
"""Generates KERN's own file and folder icons: assets/icons/ft-*.svg and the Rust
table that embeds them. One grid, one stroke weight. Color comes from the theme at
draw time, so the same set works in both themes. Run after changing a shape."""
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "assets/icons"
SW = 1.25


def svg(body, mask=""):
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" '
        f'fill="none" stroke="currentColor" stroke-width="{SW}" stroke-linecap="round" '
        f'stroke-linejoin="round">{mask}{body}</svg>\n'
    )


# A small stroke alphabet on a unit box.
FONT = {
    "C": [[(1, 0.1), (0.75, 0), (0.25, 0), (0, 0.25), (0, 0.75), (0.25, 1), (0.75, 1), (1, 0.9)]],
    "D": [[(0, 0), (0.6, 0), (1, 0.3), (1, 0.7), (0.6, 1), (0, 1), (0, 0)]],
    "G": [[(1, 0.1), (0.75, 0), (0.25, 0), (0, 0.25), (0, 0.75), (0.25, 1), (0.75, 1), (1, 0.8), (1, 0.55), (0.55, 0.55)]],
    "H": [[(0, 0), (0, 1)], [(1, 0), (1, 1)], [(0, 0.5), (1, 0.5)]],
    "J": [[(1, 0), (1, 0.75), (0.75, 1), (0.25, 1), (0, 0.75)]],
    "K": [[(0, 0), (0, 1)], [(1, 0), (0, 0.55), (1, 1)]],
    "L": [[(0, 0), (0, 1), (1, 1)]],
    "O": [[(0.25, 0), (0.75, 0), (1, 0.25), (1, 0.75), (0.75, 1), (0.25, 1), (0, 0.75), (0, 0.25), (0.25, 0)]],
    "P": [[(0, 1), (0, 0), (0.75, 0), (1, 0.2), (1, 0.4), (0.75, 0.6), (0, 0.6)]],
    "R": [[(0, 1), (0, 0), (0.75, 0), (1, 0.2), (1, 0.4), (0.75, 0.58), (0, 0.58)], [(0.55, 0.58), (1, 1)]],
    "S": [[(1, 0.12), (0.75, 0), (0.25, 0), (0, 0.2), (0, 0.35), (0.25, 0.5), (0.75, 0.5), (1, 0.65), (1, 0.8), (0.75, 1), (0.25, 1), (0, 0.88)]],
    "T": [[(0, 0), (1, 0)], [(0.5, 0), (0.5, 1)]],
    "V": [[(0, 0), (0.5, 1), (1, 0)]],
    "W": [[(0, 0), (0.25, 1), (0.5, 0.35), (0.75, 1), (1, 0)]],
    "Y": [[(0, 0), (0.5, 0.5), (1, 0)], [(0.5, 0.5), (0.5, 1)]],
    "+": [[(0.5, 0.15), (0.5, 0.85)], [(0.15, 0.5), (0.85, 0.5)]],
}
TILE = "M3 1.75h10A1.25 1.25 0 0 1 14.25 3v10A1.25 1.25 0 0 1 13 14.25H3A1.25 1.25 0 0 1 1.75 13V3A1.25 1.25 0 0 1 3 1.75z"


def letters_path(text, y=5.0, h=6.0):
    if len(text) == 1:
        slots = [(5.1, 5.8)]
    else:
        slots = [(3.2, 4.0), (8.8, 4.0)]
    d = ""
    for ch, (x, w) in zip(text, slots):
        for stroke in FONT[ch]:
            pts = [f"{x + px * w:.2f} {y + py * h:.2f}" for px, py in stroke]
            d += "M" + "L".join(pts)
    return d


def tile_solid(text):
    """A filled tile with the letters cut out of it: reads at a glance."""
    name = "k"
    mask = (
        f'<mask id="{name}" maskUnits="userSpaceOnUse" x="0" y="0" width="16" height="16">'
        '<rect width="16" height="16" fill="#fff" stroke="none"/>'
        f'<path d="{letters_path(text)}" stroke="#000" stroke-width="1.45"/></mask>'
    )
    return svg(f'<path d="{TILE}" fill="currentColor" stroke="none" mask="url(#{name})"/>', mask)


def tile_outline(text):
    """The configuration or declaration sibling of a language: same letters, hollow."""
    return svg(f'<path d="{TILE}" fill="currentColor" fill-opacity=".14"/><path d="{letters_path(text)}" stroke-width="1.3"/>')


def gear(r=2.1, teeth=8, outer=6.3, inner=4.7):
    d = ""
    for i in range(teeth):
        a = i * 2 * math.pi / teeth
        d += f"M{8 + inner * math.cos(a):.2f} {8 + inner * math.sin(a):.2f}L{8 + outer * math.cos(a):.2f} {8 + outer * math.sin(a):.2f}"
    return (
        f'<circle cx="8" cy="8" r="{inner}" fill="currentColor" fill-opacity=".14"/>'
        f'<circle cx="8" cy="8" r="{r}"/><path d="{d}"/>'
    )


def atom():
    e = '<ellipse cx="8" cy="8" rx="6.4" ry="2.5"{}/>'
    return (
        '<circle cx="8" cy="8" r="1.25" fill="currentColor" stroke="none"/>'
        + e.format("")
        + e.format(' transform="rotate(60 8 8)"')
        + e.format(' transform="rotate(120 8 8)"')
    )


FILE_OUTLINE = '<path d="M4 1.75h5.25L12.75 5.25V13A1.25 1.25 0 0 1 11.5 14.25h-7.5A1.25 1.25 0 0 1 2.75 13V3A1.25 1.25 0 0 1 4 1.75z" fill="currentColor" fill-opacity=".12"/><path d="M9.25 1.75v3.5h3.5"/>'
CUBE = '<path d="M8 1.75 13.75 5v6L8 14.25 2.25 11V5z" fill="currentColor" fill-opacity=".14"/><path d="M2.25 5 8 8.25 13.75 5M8 8.25v6"/>'

SHAPES = {
    "ft-js": tile_solid("JS"),
    "ft-ts": tile_solid("TS"),
    "ft-py": tile_solid("PY"),
    "ft-swift": tile_solid("SW"),
    "ft-c": tile_solid("C"),
    "ft-cpp": tile_solid("C+"),
    "ft-h": tile_outline("H"),
    "ft-go": tile_solid("GO"),
    "ft-java": tile_solid("JV"),
    "ft-kotlin": tile_solid("K"),
    "ft-ruby": tile_solid("R"),
    "ft-php": tile_solid("P"),
    "ft-lua": tile_solid("L"),
    "ft-dts": tile_outline("D"),
    "ft-yaml": tile_outline("Y"),
    "ft-toml": tile_outline("T"),
    "ft-tsconfig": tile_outline("TS"),
    "ft-jsconfig": tile_outline("JS"),
    "ft-react": svg(atom()),
    "ft-rust": svg(
        gear(r=0.1, teeth=10, outer=6.5, inner=5.1).replace('<circle cx="8" cy="8" r="0.1"/>', "")
        + f'<path d="{letters_path("R", y=5.3, h=5.4).replace("5.10", "5.70")}" stroke-width="1.15" transform="translate(.0 0) scale(1)"/>'
    ),
    "ft-html": svg('<path d="M5.25 4 1.75 8l3.5 4M10.75 4l3.5 4-3.5 4M9.2 2.75l-2.4 10.5"/>'),
    "ft-css": svg('<path d="M6.25 2 4.5 14M11.5 2 9.75 14M2.5 5.75h11.5M2 10.25h11.5" stroke-width="1.4"/>'),
    "ft-json": svg('<path d="M5.75 2H5a1.75 1.75 0 0 0-1.75 1.75v2.5C3.25 7.25 2.5 8 1.75 8c.75 0 1.5.75 1.5 1.75v2.5A1.75 1.75 0 0 0 5 14h.75M10.25 2H11a1.75 1.75 0 0 1 1.75 1.75v2.5c0 1 .75 1.75 1.5 1.75-.75 0-1.5.75-1.5 1.75v2.5A1.75 1.75 0 0 1 11 14h-.75" stroke-width="1.4"/><circle cx="8" cy="8" r=".9" fill="currentColor" stroke="none"/>'),
    "ft-md": svg('<rect x="1.25" y="3.25" width="13.5" height="9.5" rx="1.5" fill="currentColor" fill-opacity=".14"/><path d="M3.5 10.25v-4.5l2 2.25 2-2.25v4.5M11.25 5.75v4.5M9.5 8.5l1.75 1.75L13 8.5"/>'),
    "ft-shell": svg('<rect x="1.75" y="2.25" width="12.5" height="11.5" rx="1.5" fill="currentColor" fill-opacity=".14"/><path d="m4.25 5.75 2.5 2.25-2.5 2.25M8.25 10.75h3.5"/>'),
    "ft-inno": svg('<path d="M2 9.5v3.25a1.25 1.25 0 0 0 1.25 1.25h9.5A1.25 1.25 0 0 0 14 12.75V9.5" fill="currentColor" fill-opacity=".14"/><path d="M8 1.75v8.5M4.5 7 8 10.5 11.5 7"/>'),
    "ft-image": svg('<rect x="1.75" y="2.5" width="12.5" height="11" rx="1.5" fill="currentColor" fill-opacity=".14"/><circle cx="5.5" cy="6" r="1.15"/><path d="m2 12 3.75-3.25 2.5 2 2.5-2.75 3.5 3.75"/>'),
    "ft-video": svg('<rect x="1.75" y="2.5" width="12.5" height="11" rx="1.5" fill="currentColor" fill-opacity=".14"/><path d="M6.5 5.5v5L10.75 8z" fill="currentColor"/>'),
    "ft-vite": svg('<path d="M9.25 1.25 3 9.25h4.25L6.5 14.75l6.5-8.5H8.5z" fill="currentColor" fill-opacity=".85"/>'),
    "ft-package": svg(CUBE),
    "ft-package-lock": svg('<rect x="2.5" y="6.75" width="11" height="7.5" rx="1.25" fill="currentColor" fill-opacity=".14"/><path d="M5 6.75V4.75a3 3 0 0 1 6 0v2M2.5 10.25h11"/><circle cx="8" cy="10.25" r="1" fill="currentColor" stroke="none"/>'),
    "ft-git": svg('<path d="M8 1.5 14.5 8 8 14.5 1.5 8z" fill="currentColor" fill-opacity=".14"/><path d="M6.25 4.75 11 9.5M8 6.5v4"/><circle cx="8" cy="11" r=".7" fill="currentColor"/><circle cx="11" cy="9.5" r=".7" fill="currentColor"/>'),
    "ft-env": svg('<path d="M2 4.25h5.25M10.5 4.25H14M2 8h1.75M7 8h7M2 11.75h6.75M12 11.75h2"/><circle cx="8.9" cy="4.25" r="1.6"/><circle cx="5.4" cy="8" r="1.6"/><circle cx="10.4" cy="11.75" r="1.6"/>'),
    "ft-key": svg('<circle cx="5.5" cy="10.5" r="3.25" fill="currentColor" fill-opacity=".14"/><path d="m7.9 8.1 5.85-5.85M11 5l2 2M9.25 6.75l1.5 1.5"/>'),
    "ft-readme": svg('<path d="M8 4.25C6.75 3 4.75 2.75 1.75 2.75v9.75c3 0 5 .25 6.25 1.5 1.25-1.25 3.25-1.5 6.25-1.5V2.75c-3 0-5 .25-6.25 1.5z" fill="currentColor" fill-opacity=".14"/><path d="M8 4.25V14"/>'),
    "ft-license": svg(FILE_OUTLINE + '<circle cx="7.75" cy="9.25" r="2"/><path d="M6.75 11v2.25l1-.6 1 .6V11"/>'),
    "ft-docker": svg('<path d="M1.5 9.25h10.25c.4-1 1.6-1.6 2.75-1-.3 3.75-2.75 5.75-6.75 5.75-3.5 0-5.5-1.75-6.25-4.75z" fill="currentColor" fill-opacity=".14"/><path d="M3.5 6.75h2v2.5h-2zM6 6.75h2v2.5H6zM8.5 6.75h2v2.5h-2zM6 4h2v2.75H6z" stroke-width="1"/>'),
    "ft-config": svg(gear()),
    "ft-make": svg('<path d="m9.5 2.25 4.25 4.25-2 2-1.25-1.25-6.25 6.25a1.4 1.4 0 0 1-2-2L8.5 5.25 7.25 4z" fill="currentColor" fill-opacity=".14"/>'),
    "ft-cmake": svg('<path d="M8 1.75 14.5 14H1.5z" fill="currentColor" fill-opacity=".14"/><path d="M8 1.75v7.75M1.5 14 8 9.5l6.5 4.5"/>'),
    "ft-text": svg(FILE_OUTLINE + '<path d="M5.25 8h5.5M5.25 10.75h5.5"/>'),
    "ft-file": svg(FILE_OUTLINE),
    "ft-test": svg('<path d="M6 1.75h4M6.75 1.75v4.5L2.75 12.5A1.2 1.2 0 0 0 3.75 14.25h8.5a1.2 1.2 0 0 0 1-1.75l-4-6.25v-4.5" fill="currentColor" fill-opacity=".14"/><path d="M4.75 10h6.5"/>'),
    "ft-archive": svg('<rect x="1.75" y="2.5" width="12.5" height="3.25" rx="1"/><path d="M2.75 5.75v6.5a1.25 1.25 0 0 0 1.25 1.25h8a1.25 1.25 0 0 0 1.25-1.25v-6.5" fill="currentColor" fill-opacity=".14"/><path d="M6.5 8.5h3"/>'),
    "ft-database": svg('<ellipse cx="8" cy="3.75" rx="5.5" ry="2"/><path d="M2.5 3.75v8.5c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2v-8.5" fill="currentColor" fill-opacity=".14"/><path d="M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>'),
}

FOLDER_BODY = '<path d="M1.75 4.25a1.25 1.25 0 0 1 1.25-1.25h3.1l1.6 1.75H13a1.25 1.25 0 0 1 1.25 1.25v6A1.25 1.25 0 0 1 13 13.25H3a1.25 1.25 0 0 1-1.25-1.25z" fill="currentColor" fill-opacity=".22"/>'
FOLDER_OPEN = '<path d="M1.75 12.25v-8A1.25 1.25 0 0 1 3 3h3.1l1.6 1.75H12a1.25 1.25 0 0 1 1.25 1.25v1"/><path d="M1.75 12.5 3.3 7.9A1.25 1.25 0 0 1 4.5 7h9.45a.75.75 0 0 1 .7 1l-1.45 4.4a1.25 1.25 0 0 1-1.2.85H3a1.25 1.25 0 0 1-1.25-.75z" fill="currentColor" fill-opacity=".22"/>'
MARKS = {
    "": "",
    "src": '<path d="M6.5 7.25 4.75 9.1l1.75 1.85M9.5 7.25l1.75 1.85-1.75 1.85" stroke-width="1.15"/>',
    "public": '<circle cx="8" cy="9.1" r="2.35" stroke-width="1.05"/><path d="M5.75 9.1h4.5M8 6.75c1.3 1.5 1.3 3.2 0 4.7-1.3-1.5-1.3-3.2 0-4.7z" stroke-width=".85"/>',
    "assets": '<path d="M8 6.5 8.75 8.3 10.6 9.1 8.75 9.9 8 11.75 7.25 9.9 5.4 9.1l1.85-.8z" fill="currentColor" stroke="none"/>',
    "tests": '<path d="m5.5 9.25 1.75 1.75 3.25-3.5" stroke-width="1.3"/>',
    "server": '<path d="M5.5 7.75h5M5.5 9.25h5M5.5 10.75h5" stroke-width="1.1"/>',
    "dist": '<path d="M8 6.75v4.25M6.25 9.5 8 11.25 9.75 9.5" stroke-width="1.2"/>',
    "node": '<path d="M8 6.5 10.25 7.8v2.6L8 11.7l-2.25-1.3V7.8z" stroke-width="1.05"/>',
}

names = []
for name, body in SHAPES.items():
    (OUT / f"{name}.svg").write_text(body)
    names.append(name)
for mark, body in MARKS.items():
    suffix = f"-{mark}" if mark else ""
    for state, base in (("closed", FOLDER_BODY), ("open", FOLDER_OPEN)):
        name = f"ft-folder-{state}{suffix}"
        # The open folder's front sits lower: its mark moves with it.
        mark_body = f'<g transform="translate(.35 .6)">{body}</g>' if state == "open" and body else body
        (OUT / f"{name}.svg").write_text(svg(base + mark_body))
        names.append(name)

rows = "\n".join(
    f'        "icons/{n}.svg" => Some(include_bytes!("../../assets/icons/{n}.svg")),' for n in sorted(names)
)
(ROOT / "src/ui/file_icon_assets.rs").write_text(
    "//! Generated by scripts/make_file_icons.py. Do not edit.\n"
    "pub fn embedded(path: &str) -> Option<&'static [u8]> {\n    match path {\n" + rows + "\n        _ => None,\n    }\n}\n"
)
print(len(names), "icons")
