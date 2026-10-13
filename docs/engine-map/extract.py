#!/usr/bin/env python3
"""Engine map extraction.

Builds docs/engine-map/engine-map.html from two layers:

- map.authored.json, written by a person: regions, subsystems, connections,
  lenses and notes. Subsystems claim source files by path pattern.
- the code itself: each file's line count and the first paragraph of its //!
  doc, plus the cross-file references that show which subsystems actually use
  each other.

Then it checks the authored layer against the code and reports drift: files no
subsystem claims, claims that match nothing, names in backticks that no longer
exist, lens code references that moved, and couplings with no described
connection.

Usage:  python docs/engine-map/extract.py [--strict] [--root PATH]
Errors fail. Warnings report; --strict makes them fail too.
Standard library only.
"""
import argparse
import fnmatch
import json
import re
import subprocess
import sys
from datetime import date
from pathlib import Path

SOURCE_GLOBS = ["crates/*/src/**/*.rs", "shaders/*.wgsl", "assets/**/*", "palettes/*"]
COUPLING_MIN_REFS = 5  # below this, an undescribed coupling is noise

WORD = re.compile(r"[A-Za-z_]\w*")
ITEM = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait|type|fn|const|static|mod)\s+([A-Za-z_]\w*)", re.M)
USE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([^;]+);", re.M)
DOC_LINK = re.compile(r"\[`([^`]+)`\](?:\([^)]*\)|\[[^\]]*\])?")
MD_LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")
TICKS = re.compile(r"`([^`]+)`")


# ------------------------------------------------------------------ scanning

def scan(root):
    """Every mapped source file, keyed by repo-relative posix path."""
    files = {}
    for pattern in SOURCE_GLOBS:
        for p in root.glob(pattern):
            if p.is_file():
                files[p.relative_to(root).as_posix()] = p
    return dict(sorted(files.items()))


def lang_of(path):
    if path.endswith(".rs"):
        return "rs"
    if path.endswith(".wgsl"):
        return "wgsl"
    return "asset"


def module_doc(text):
    """First prose paragraph of a file's leading //! block, or ''."""
    lines = []
    for raw in text.splitlines():
        s = raw.strip()
        if s.startswith("//!"):
            lines.append(s[3:].strip())
        elif lines:
            break
        elif s and not s.startswith("#!["):
            return ""
    paragraphs, cur = [], []
    for line in lines + [""]:
        if line:
            cur.append(line)
        elif cur:
            paragraphs.append(" ".join(cur))
            cur = []
    for para in paragraphs:
        if para.startswith(("#", "```")):
            continue
        para = DOC_LINK.sub(r"`\1`", para)
        para = MD_LINK.sub(r"\1", para).replace("**", "")
        sentences = re.split(r"(?<=[.:])\s+", para)
        return " ".join(sentences[:2]) if len(para) > 260 else para
    return ""


def crate_table(root):
    """Rust identifier of each crate ('nodegraph_eval') to its src directory."""
    table = {}
    for toml in root.glob("crates/*/Cargo.toml"):
        m = re.search(r"\[package\][^\[]*?\bname\s*=\s*\"([^\"]+)\"", toml.read_text(encoding="utf-8"), re.S)
        if m:
            table[m.group(1).replace("-", "_")] = toml.parent / "src"
    return table


# --------------------------------------------------------- reference graph

def module_file(src, mods):
    """The file that defines module path `mods` in the crate rooted at `src`."""
    if not mods:
        for name in ("lib.rs", "main.rs"):
            if (src / name).exists():
                return src / name
        return None
    base = src.joinpath(*mods)
    for cand in (base.with_suffix(".rs"), base / "mod.rs"):
        if cand.exists():
            return cand
    return None


def module_path(path, src):
    """Module segments of a source file: src/a/b.rs -> [a, b]."""
    parts = list(path.relative_to(src).with_suffix("").parts)
    if parts[-1] == "mod" or parts in (["lib"], ["main"]):
        parts = parts[:-1]
    return parts


def expand_use(tree):
    """`a::{b, c::{d, self}}` -> [a::b, a::c::d, a::c]."""
    tree = re.sub(r"\s+as\s+\w+", "", tree)
    tree = re.sub(r"\s+", "", tree)
    if "{" not in tree:
        return [tree]
    if "}" not in tree:
        return []
    i = tree.index("{")
    prefix, body = tree[:i], tree[i + 1:tree.rindex("}")]
    parts, depth, cur = [], 0, ""
    for ch in body:
        if ch == "," and depth == 0:
            parts.append(cur)
            cur = ""
            continue
        depth += (ch == "{") - (ch == "}")
        cur += ch
    parts.append(cur)
    out = []
    for part in filter(None, parts):
        out.extend([prefix.rstrip(":")] if part == "self" else expand_use(prefix + part))
    return out


def references(path, text, crates, defs):
    """Files `path` refers to through `crate::`, `super::`, `self::` or another
    workspace crate. Returns {target file: count}."""
    src = next((s for s in crates.values() if s in path.parents), None)
    if src is None:
        return {}
    here = module_path(path, src)
    roots = "|".join(["crate", "super", "self"] + sorted(crates))
    inline = re.compile(rf"\b({roots})((?:::\w+)+)")
    paths = [p for tree in USE.findall(text) for p in expand_use(tree)]
    paths += [m.group(1) + m.group(2) for m in inline.finditer(USE.sub("", text))]

    counts = {}
    for p in paths:
        segs = [s for s in p.split("::") if s and s != "*"]
        if not segs:
            continue
        head, rest = segs[0], segs[1:]
        if head == "crate":
            base, start = src, []
        elif head == "self":
            base, start = src, list(here)
        elif head == "super":
            base, start = src, list(here[:-1])
            while rest and rest[0] == "super":
                start, rest = start[:-1], rest[1:]
        elif head in crates:
            base, start = crates[head], []
        else:
            continue
        mods, target = list(start), module_file(base, start)
        for seg in rest:
            f = module_file(base, mods + [seg])
            if not f:
                break
            mods.append(seg)
            target = f
        if mods == start and rest:
            # Nothing past the root resolved: likely a re-exported item.
            owners = [f for f in defs.get(rest[0], ()) if base in f.parents]
            if len(owners) == 1:
                target = owners[0]
        if target and target != path:
            counts[target] = counts.get(target, 0) + 1
    return counts


# ------------------------------------------------------------------- claims

def specificity(pattern):
    literal = re.split(r"[*?\[]", pattern)[0]
    return (pattern == literal, len(literal))


def assign(files, subs, health):
    """Give each file to the most specific claim that matches it."""
    owner, used = {}, set()
    for path in files:
        best, best_subs = None, []
        for sub in subs:
            for pattern in sub.get("claims", []):
                if fnmatch.fnmatchcase(path, pattern):
                    used.add((sub["id"], pattern))
                    score = specificity(pattern)
                    if best is None or score > best:
                        best, best_subs = score, [sub["id"]]
                    elif score == best and sub["id"] not in best_subs:
                        best_subs.append(sub["id"])
        if not best_subs:
            health["warnings"].append(issue(f"`{path}` belongs to no subsystem.", []))
        else:
            owner[path] = best_subs[0]
            if len(best_subs) > 1:
                health["warnings"].append(issue(f"`{path}` is claimed equally by {', '.join(best_subs)}.", best_subs))
    for sub in subs:
        for pattern in sub.get("claims", []):
            if (sub["id"], pattern) not in used:
                health["warnings"].append(issue(f"Claim `{pattern}` matches no file.", [sub["id"]]))
    return owner


# ------------------------------------------------------------------- checks

def issue(text, nodes):
    return {"text": text, "nodes": nodes}


def stale_tokens(text, words, file_names, rs_text):
    """Backticked names in authored text that no longer appear in the code."""
    missing = []
    for tok in TICKS.findall(text):
        if tok.startswith("--"):
            found = tok in rs_text
        elif "/" in tok or tok.startswith(".") or re.search(r"\.(rs|json|ron|wgsl)$", tok):
            found = any(tok in f for f in file_names)
        else:
            found = all(w in words for w in WORD.findall(tok))
        if not found:
            missing.append(tok)
    return missing


def check_code_ref(ref, files, file_words):
    """'world/streaming.rs · Type::method (note)' -> (resolved path or None, ok)."""
    file_part, _, symbol = ref.partition(" · ")
    symbol = re.sub(r"\(.*?\)", "", symbol)
    for path in files:
        short = path.replace("/src/", "/").removeprefix("crates/")
        if short == file_part or short.endswith("/" + file_part):
            if all(w in file_words[path] for w in WORD.findall(symbol)):
                return path, True
    return None, False


# --------------------------------------------------------------------- main

def git(root, *args):
    try:
        return subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ""


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    ap.add_argument("--strict", action="store_true", help="fail on warnings too")
    args = ap.parse_args()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    root = args.root.resolve()
    map_dir = root / "docs" / "engine-map"
    authored = json.loads((map_dir / "map.authored.json").read_text(encoding="utf-8"))
    template = (map_dir / "viewer.html").read_text(encoding="utf-8")
    health = {"errors": [], "warnings": [], "info": []}

    # Authored layer: structural integrity.
    regions = {r["id"] for r in authored["regions"]}
    subs = authored["subsystems"]
    ids = [s["id"] for s in subs]
    for dup in {i for i in ids if ids.count(i) > 1}:
        health["errors"].append(issue(f"Subsystem id `{dup}` is used twice.", [dup]))
    for s in subs:
        if s["region"] not in regions:
            health["errors"].append(issue(f"{s['id']} names unknown region {s['region']}.", [s["id"]]))
        if s["status"] not in ("settled", "interim", "active"):
            health["errors"].append(issue(f"{s['id']} has unknown status {s['status']}.", [s["id"]]))
    refs = [("connection", n) for e in authored["edges"] for n in (e["from"], e["to"])]
    refs += [("lens step", n) for lens in authored["lenses"].values() for st in lens["steps"] for n in st["nodes"]]
    refs += [("note", n) for note in authored["notes"] for n in note["nodes"]]
    for kind, n in refs:
        if n not in ids:
            health["errors"].append(issue(f"A {kind} points at unknown subsystem {n}.", []))

    # Code layer.
    files = scan(root)
    texts = {p: f.read_text(encoding="utf-8", errors="replace") for p, f in files.items() if lang_of(p) != "asset"}
    file_words = {p: set(WORD.findall(t)) for p, t in texts.items()}
    words = set().union(*file_words.values())
    rs_text = "\n".join(t for p, t in texts.items() if p.endswith(".rs"))
    crates = crate_table(root)
    defs = {}
    for p, t in texts.items():
        if p.endswith(".rs"):
            for name in ITEM.findall(t):
                defs.setdefault(name, []).append(files[p])

    owner = assign(files, subs, health)
    notes = authored.get("module_notes", {})
    for path in notes:
        if path not in files:
            health["warnings"].append(issue(f"Module note for `{path}`, which no longer exists.", []))

    modules = {s["id"]: [] for s in subs}
    undocumented = []
    for path, sub_id in owner.items():
        lang = lang_of(path)
        text = texts.get(path, "")
        doc = module_doc(text) if lang == "rs" else ""
        if lang == "rs" and not doc and path not in notes:
            undocumented.append(path)
        if lang != "asset" and not text.strip():
            health["warnings"].append(issue(f"`{path}` is empty.", [sub_id]))
        modules[sub_id].append({
            "p": path, "lang": lang,
            "l": text.count("\n") + (1 if text and not text.endswith("\n") else 0) if lang != "asset" else None,
            "d": doc or notes.get(path, ""),
            "src": "doc" if doc else ("note" if path in notes else ""),
        })
    if undocumented:
        health["info"].append(issue(f"{len(undocumented)} Rust files have neither a //! doc nor a module note: "
                                    + ", ".join(f"`{p}`" for p in undocumented), sorted({owner[p] for p in undocumented})))

    # Couplings between subsystems, from cross-file references.
    by_path = {f: p for p, f in files.items()}
    pair_refs = {}
    for p, t in texts.items():
        if not p.endswith(".rs") or p not in owner:
            continue
        for target, n in references(files[p], t, crates, defs).items():
            tp = by_path.get(target)
            a, b = owner[p], owner.get(tp)
            if b and a != b:
                pair_refs[(a, b)] = pair_refs.get((a, b), 0) + n
    described = {frozenset((e["from"], e["to"])) for e in authored["edges"]}
    couplings = [{"user": a, "used": b, "refs": n, "described": frozenset((a, b)) in described}
                 for (a, b), n in sorted(pair_refs.items(), key=lambda kv: -kv[1])]
    foundations = {s["id"] for s in subs if s.get("foundation")}
    for c in couplings:
        if not c["described"] and c["refs"] >= COUPLING_MIN_REFS and c["used"] not in foundations:
            health["info"].append(issue(f"{c['user']} refers to {c['used']} {c['refs']} times in code, but no connection between them is described.",
                                        [c["user"], c["used"]]))

    # Authored text against the code.
    file_names = list(files)
    def check_text(where, text, nodes):
        for tok in stale_tokens(text, words, file_names, rs_text):
            health["warnings"].append(issue(f"{where} mentions `{tok}`, which is not in the code.", nodes))
    for s in subs:
        for field in ("short", "summary", "runs", "statusNote"):
            check_text(f"{s['name']} ({field})", s.get(field, ""), [s["id"]])
        for field in ("owns", "rules"):
            for item in s.get(field, []):
                check_text(f"{s['name']} ({field})", item, [s["id"]])
    for e in authored["edges"]:
        check_text(f"Connection {e['from']} -> {e['to']}", e["label"], [e["from"], e["to"]])
    for path, note in notes.items():
        check_text(f"Module note for `{path}`", note, [owner[path]] if path in owner else [])
    lenses = {}
    for key, lens in authored["lenses"].items():
        steps = []
        for i, st in enumerate(lens["steps"]):
            check_text(f"{lens['name']} step {i + 1}", st["text"], st["nodes"])
            code = []
            for ref in st["code"]:
                path, ok = check_code_ref(ref, files, file_words)
                if not ok:
                    health["warnings"].append(issue(f"{lens['name']} step {i + 1} points at `{ref}`, which was not found.", st["nodes"]))
                code.append({"ref": ref, "path": path, "ok": ok})
            steps.append({**st, "code": code})
        lenses[key] = {**lens, "steps": steps}

    # Assemble.
    rust = sum(m["l"] or 0 for ms in modules.values() for m in ms if m["lang"] == "rs")
    wgsl = sum(m["l"] or 0 for ms in modules.values() for m in ms if m["lang"] == "wgsl")
    dirty = bool(git(root, "status", "--porcelain", "--", "crates", "shaders", "assets"))
    data = {
        "meta": {"commit": git(root, "rev-parse", "--short", "HEAD") or "unknown",
                 "branch": git(root, "rev-parse", "--abbrev-ref", "HEAD") or "unknown",
                 "dirty": dirty, "date": date.today().isoformat(),
                 "rust": rust, "wgsl": wgsl, "crates": len(crates), "files": len(owner)},
        "regions": authored["regions"],
        "subs": [{k: v for k, v in s.items() if k != "claims"} | {"modules": sorted(modules[s["id"]], key=lambda m: (-(m["l"] or 0), m["p"]))}
                 for s in subs],
        "edges": authored["edges"],
        "lenses": lenses,
        "notes": authored["notes"],
        "couplings": couplings,
        "health": health,
    }
    marker = "/*MAP_DATA*/"
    if template.count(marker) != 1:
        health["errors"].append(issue(f"viewer.html must contain {marker} exactly once.", []))
    else:
        payload = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
        (map_dir / "engine-map.html").write_text(template.replace(marker, payload), encoding="utf-8")

    # Report.
    print(f"Engine map: {len(subs)} subsystems, {len(owner)} files, {rust:,} lines of Rust, {wgsl:,} of WGSL"
          f" at {data['meta']['commit']}{' (uncommitted changes)' if dirty else ''}")
    for level in ("errors", "warnings", "info"):
        items = health[level]
        print(f"\n{level} ({len(items)})")
        for it in items:
            print(f"  - {it['text']}")
    if not health["errors"]:
        print(f"\nWrote {(map_dir / 'engine-map.html').relative_to(root).as_posix()}")
    failed = health["errors"] or (args.strict and health["warnings"])
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())