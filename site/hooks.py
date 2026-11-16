"""Build-time glue for the jotty website.

The site has no copies of the documentation: this hook adds docs/*.md,
CHANGELOG.md and the logo as pages of the site, fills `<!-- readme: X -->`
markers with the README section headed X and `<!-- screenshot: X -->`
markers with assets/screenshots/X once it exists, rewrites links that point
outside the site to GitHub, and generates the config reference from
docs/config.example.toml.
"""

import posixpath
import re
from pathlib import Path

from mkdocs.structure.files import File

REPO_URL = "https://github.com/justanoobcoder/jotty"
ROOT = Path(__file__).resolve().parent.parent

# Site page -> repository file it comes from.
SOURCES = {
    "usage.md": "docs/usage.md",
    "configuration.md": "docs/configuration.md",
    "known-issues.md": "docs/known-issues.md",
    "architecture.md": "docs/architecture.md",
    "development.md": "docs/development.md",
    "changelog.md": "CHANGELOG.md",
    "assets/logo.svg": "extra/jotty.svg",
}
GENERATED = {"config-reference.md": "docs/config.example.toml"}
# Repository files that are pages of the site under another name.
SITE_PAGE = {repo: page for page, repo in {**SOURCES, **GENERATED}.items()}
SITE_PAGE["README.md#install"] = "install.md"
SITE_PAGE["README.md#keybinds"] = "features.md#keybinds"

# Pages filled from README sections, for the edit link.
README_PAGES = {"install.md"}

LINK = re.compile(r"\]\(([^)\s]+)\)")
README_MARKER = re.compile(r"^<!-- readme: (.+?) -->$", re.M)
SCREENSHOT_MARKER = re.compile(r"^<!-- screenshot: (\S+) -->$", re.M)


def on_files(files, config):
    for page, repo in SOURCES.items():
        files.append(File.generated(config, page, abs_src_path=str(ROOT / repo)))
    for page, repo in GENERATED.items():
        content = config_reference((ROOT / repo).read_text())
        files.append(File.generated(config, page, content=content))
    return files


def on_serve(server, config, builder):
    for path in ("docs", "CHANGELOG.md", "README.md"):
        server.watch(str(ROOT / path))
    return server


def on_page_markdown(markdown, page, config, files):
    src = page.file.src_uri
    if src in SOURCES:
        markdown = rewrite_links(markdown, posixpath.dirname(SOURCES[src]))
    if src == "changelog.md":
        # CHANGELOG.md has a setext title in capitals.
        markdown = re.sub(r"\ACHANGELOG\n=+\n", "# Changelog\n", markdown)
    markdown = README_MARKER.sub(lambda m: readme_section(m.group(1)), markdown)
    return SCREENSHOT_MARKER.sub(lambda m: screenshot(m, page, config), markdown)


def screenshot(m, page, config):
    """The image a `<!-- screenshot: name -->` marker names, once it exists
    in assets/screenshots/; until then the marker stays a comment."""
    name = m.group(1)
    if not (Path(config.docs_dir) / "assets/screenshots" / name).is_file():
        return m.group(0)
    alt = Path(name).stem.replace("-", " ")
    if name.endswith((".mp4", ".webm")):
        # Raw HTML: MkDocs doesn't rewrite its URLs, so make it relative to
        # the page's own URL. Muted, so browsers autoplay it like a GIF.
        src = posixpath.relpath(f"assets/screenshots/{name}", page.url or ".")
        return (
            f'<video class="jotty-shot" src="{src}" title="{alt}" '
            "autoplay loop muted playsinline></video>"
        )
    return f'![{alt}](assets/screenshots/{name}){{ .jotty-shot loading="lazy" }}'


def on_page_context(context, page, config, nav):
    src = page.file.src_uri
    repo = {**SOURCES, **GENERATED}.get(src)
    if src in README_PAGES:
        repo = "README.md"
    if repo:
        page.edit_url = f"{REPO_URL}/edit/main/{repo}"
    return context


def rewrite_links(markdown, base):
    """Point links relative to `base` (a repository directory) at the site
    page for that file, or at the file on GitHub."""

    def repl(m):
        target = m.group(1)
        if re.match(r"[a-z]+:|#", target):
            return m.group(0)
        path, _, anchor = target.partition("#")
        path = posixpath.normpath(posixpath.join(base, path))
        if page := SITE_PAGE.get(f"{path}#{anchor}"):
            return f"]({page})"
        if page := SITE_PAGE.get(path):
            return f"]({page}" + (f"#{anchor}" if anchor else "") + ")"
        url = f"{REPO_URL}/blob/main/{path}" + (f"#{anchor}" if anchor else "")
        return f"]({url})"

    return LINK.sub(repl, markdown)


def readme_section(title):
    """The body of the README section headed `title`, with its subheadings
    raised so the first level below it becomes `##`."""
    lines = (ROOT / "README.md").read_text().splitlines()
    for start, line in enumerate(lines):
        m = re.match(r"(#+) (.*)", line)
        if m and m.group(2) == title:
            level = len(m.group(1))
            break
    else:
        raise ValueError(f"README.md has no section {title!r}")
    body = []
    in_code = False
    for line in lines[start + 1 :]:
        if line.startswith("```"):
            in_code = not in_code
        m = None if in_code else re.match(r"(#+) ", line)
        if m:
            if len(m.group(1)) <= level:
                break
            line = "#" * (len(m.group(1)) - level + 1) + line[len(m.group(1)) :]
        body.append(line)
    return rewrite_links("\n".join(body).strip("\n"), "")


# Config reference ---------------------------------------------------------

KEY = re.compile(r"([a-z0-9_]+) = (.*)$")


def config_reference(example):
    """Markdown tables of every option in config.example.toml, with the
    comment above each option as its description."""
    sections = parse_example(example)
    out = [
        "# Config reference",
        "",
        "Every option, with its default value, generated from",
        f"[`config.example.toml`]({REPO_URL}/blob/main/docs/config.example.toml).",
        "See the [guide](configuration.md) for how the options work together.",
        "",
        '<input id="config-filter" class="md-input" type="search" '
        'placeholder="Filter options, e.g. opacity or ctrl+shift" '
        'aria-label="Filter options">',
        "",
    ]
    # Sections such as [files.image] are grouped under their parent. Each
    # group, and each subsection, is a div the filter can hide.
    groups = {}
    for section in sections:
        groups.setdefault(section[0].partition(".")[0], []).append(section)
    for parent, members in groups.items():
        out += ['<div class="config-section" markdown>', ""]
        out += [f"## `[{parent}]`", "", f"[Guide](configuration.md#{parent})", ""]
        for name, intro, options in members:
            nested = name != parent
            if intro:
                out += [intro, ""]
            if nested:
                out += ['<div class="config-section" markdown>', ""]
                out += [f"### `[{name}]`", ""]
            out += ["| Option | Default | Description |", "| --- | --- | --- |"]
            for key, default, desc in options:
                anchor = f"{name}.{key}"
                cell = f'<a id="{anchor}" href="#{anchor}"><code>{key}</code></a>'
                value = "unset" if default is None else code(default)
                out.append(f"| {cell} | {value} | {desc} |")
            out.append("")
            if nested:
                out += ["</div>", ""]
        out += ["</div>", ""]
    return "\n".join(out)


def code(value):
    # Inline code can't hold a pipe inside a table cell; escape it.
    return "`" + value.replace("|", "\\|") + "`"


def parse_example(text):
    """[(section, intro, [(key, default or None, description)])]."""
    lines = text.splitlines()
    # Keys set for real, per section, to tell a commented-out option (unset
    # by default) from an example of an option that is set below it.
    real = {}
    section = None
    for line in lines:
        if m := re.match(r"\[(.+)\]$", line):
            section = m.group(1)
        elif (m := KEY.match(line)) and section:
            real.setdefault(section, set()).add(m.group(1))

    sections = []
    section = None
    comment = []
    examples = []
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        if line.startswith("#"):
            text = line.lstrip("#").strip()
            m = KEY.match(text)
            if m and section and m.group(1) not in real.get(section, ()):
                desc = describe(comment, [], None)
                sections[-1][2].append((m.group(1), None, desc + example(m.group(2))))
                comment = []
            elif m:
                examples.append(text)
            else:
                comment.append(text)
        elif not line.strip():
            # A comment block followed by a blank line introduces the section.
            if comment and section:
                intro = " ".join(comment)
                name, old, options = sections[-1]
                sections[-1] = (name, (old + " " + prose(intro)).strip(), options)
            comment, examples = [], []
        elif m := re.match(r"\[(.+)\]$", line):
            section = m.group(1)
            # A comment block right above a header introduces that section.
            sections.append((section, prose(" ".join(comment)), []))
            comment, examples = [], []
        elif m := KEY.match(line):
            key, value = m.group(1), m.group(2)
            # Arrays may span several lines.
            while value.count("[") > value.count("]"):
                value += " " + lines[i].strip()
                i += 1
            value, inline = split_inline_comment(value)
            value = re.sub(r"\[\s+", "[", re.sub(r",?\s*\]$", "]", value))
            sections[-1][2].append((key, value, describe(comment, examples, inline)))
            comment, examples = [], []
    return sections


def split_inline_comment(value):
    """Split `value  # comment` outside of strings."""
    quote = None
    for j, c in enumerate(value):
        if quote:
            if c == quote:
                quote = None
        elif c in "\"'":
            quote = c
        elif c == "#":
            return value[:j].strip(), value[j + 1 :].strip()
    return value.strip(), None


def describe(comment, examples, inline):
    desc = prose(" ".join(comment))
    for ex in examples:
        desc += example(ex.split(" = ", 1)[1])
    if inline:
        desc += example(inline.removeprefix("e.g.").strip())
    return desc.strip()


def prose(text):
    """Comment text as Markdown: angle brackets outside code spans are
    literal, not HTML."""
    parts = text.split("`")
    parts[::2] = [p.replace("<", "&lt;") for p in parts[::2]]
    return "`".join(parts)


def example(value):
    return f" Example: {code(value)}."
