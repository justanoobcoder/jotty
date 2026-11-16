"""Build-time glue for the jotty website.

The site has no copies of the documentation: this hook adds docs/user/,
docs/design/, docs/development.md, CHANGELOG.md and the logo as files of
the site, keeping the layout of docs/ so that links between the docs work
as they are. Links to other repository files point at GitHub, and
`<!-- screenshot: X -->` markers show assets/screenshots/X once it exists.
"""

import posixpath
import re
from pathlib import Path

from mkdocs.structure.files import File

REPO_URL = "https://github.com/justanoobcoder/jotty"
ROOT = Path(__file__).resolve().parent.parent

LINK = re.compile(r"\]\(([^)\s]+)\)")
SCREENSHOT_MARKER = re.compile(r"^<!-- screenshot: (\S+) -->$", re.M)


def sources():
    """Site path -> repository path of every file the hook adds."""
    found = {"changelog.md": "CHANGELOG.md", "assets/logo.svg": "extra/jotty.svg"}
    found["development.md"] = "docs/development.md"
    for sub in ("user", "design"):
        for path in sorted((ROOT / "docs" / sub).glob("*.md")):
            found[f"{sub}/{path.name}"] = f"docs/{sub}/{path.name}"
    return found


SOURCES = sources()
SITE_PATH = {repo: site for site, repo in SOURCES.items()}


def on_files(files, config):
    for site, repo in SOURCES.items():
        files.append(File.generated(config, site, abs_src_path=str(ROOT / repo)))
    return files


def on_serve(server, config, builder):
    for path in ("docs", "CHANGELOG.md"):
        server.watch(str(ROOT / path))
    return server


def on_page_markdown(markdown, page, config, files):
    src = page.file.src_uri
    if src in SOURCES:
        markdown = rewrite_links(markdown, SOURCES[src], src)
    if src == "changelog.md":
        # CHANGELOG.md has a setext title in capitals.
        markdown = re.sub(r"\ACHANGELOG\n=+\n", "# Changelog\n", markdown)
    return SCREENSHOT_MARKER.sub(lambda m: screenshot(m, page, config), markdown)


def on_page_context(context, page, config, nav):
    if repo := SOURCES.get(page.file.src_uri):
        page.edit_url = f"{REPO_URL}/edit/main/{repo}"
    return context


def rewrite_links(markdown, repo_file, site_file):
    """Point links in `repo_file` at the site page of their target, relative
    to `site_file`, or at the target on GitHub."""

    def repl(m):
        target = m.group(1)
        if re.match(r"[a-z]+:|#", target):
            return m.group(0)
        path, _, anchor = target.partition("#")
        path = posixpath.normpath(posixpath.join(posixpath.dirname(repo_file), path))
        anchor = f"#{anchor}" if anchor else ""
        if site := SITE_PATH.get(path):
            rel = posixpath.relpath(site, posixpath.dirname(site_file) or ".")
            return f"]({rel}{anchor})"
        return f"]({REPO_URL}/blob/main/{path}{anchor})"

    return LINK.sub(repl, markdown)


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
