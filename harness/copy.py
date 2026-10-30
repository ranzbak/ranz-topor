"""Read-only-safe project copy and sha256 tree verification."""
import hashlib
import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path


def _sha(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_tree(dir: Path) -> dict[str, str]:
    """Relative posix path -> sha256 of every file under dir."""
    dir = Path(dir)
    return {p.relative_to(dir).as_posix(): _sha(p) for p in sorted(dir.rglob("*")) if p.is_file()}


@dataclass
class ProjectCopy:
    pcb: Path
    pro: Path | None
    dru: Path | None
    src_hashes: dict = field(default_factory=dict)


def _lib_dirs(src_dir: Path) -> list[Path]:
    table = src_dir / "fp-lib-table"
    if not table.is_file():
        return []
    out = []
    for uri in re.findall(r'\(uri\s+"?([^")]+)"?\)', table.read_text(errors="replace")):
        if not uri.startswith("${KIPRJMOD}"):
            continue
        rel = uri[len("${KIPRJMOD}"):].lstrip("/\\")
        p = (src_dir / rel).resolve()
        if p.is_dir() and src_dir.resolve() in p.parents:
            out.append(p)
    return out


def copy_project(src_pcb: Path, dst_dir: Path) -> ProjectCopy:
    src_pcb = Path(src_pcb).resolve()
    src_dir = src_pcb.parent
    dst_dir = Path(dst_dir)
    dst_dir.mkdir(parents=True, exist_ok=True)
    hashes = sha256_tree(src_dir)

    def cp(p: Path) -> Path | None:
        if not p.is_file():
            return None
        d = dst_dir / p.name
        shutil.copyfile(p, d)
        return d

    pcb = cp(src_pcb)
    stem = src_pcb.stem
    pro = cp(src_dir / f"{stem}.kicad_pro")
    dru = cp(src_dir / f"{stem}.kicad_dru")
    cp(src_dir / f"{stem}.kicad_prl")
    cp(src_dir / "fp-lib-table")
    for lib in _lib_dirs(src_dir):
        shutil.copytree(lib, dst_dir / lib.relative_to(src_dir), dirs_exist_ok=True)
    return ProjectCopy(pcb=pcb, pro=pro, dru=dru, src_hashes=hashes)


def assert_unchanged(src_dir: Path, hashes: dict) -> None:
    now = sha256_tree(Path(src_dir))
    if now != hashes:
        changed = sorted(k for k in set(now) | set(hashes) if now.get(k) != hashes.get(k))
        raise RuntimeError(f"source tree modified: {changed[:10]}")
