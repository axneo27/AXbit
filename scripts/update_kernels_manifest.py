"""Refresh generated SPK metadata in the curated kernel manifest.

The input manifest decides which kernels belong to each group and in what
order. This script only reads IDs and coverage from those configured SPKs.

Usage:
    .venv/bin/python scripts/update_kernels_manifest.py
    .venv/bin/python scripts/update_kernels_manifest.py --output generated-kernels.toml
"""

import argparse
import os
import re
import tempfile
from pathlib import Path
from typing import Optional

import spiceypy as spice


SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_ROOT = SCRIPT_DIR.parent
DEFAULT_MANIFEST = PROJECT_ROOT / "kernels.toml"
DEFAULT_KERNELS_DIR = PROJECT_ROOT / "spice-tools" / "kernels"

KERNEL_ENTRY_RE = re.compile(
    r'^(?P<indent>\s*)\{\s*file\s*=\s*"(?P<file>[^"]+)".*\},(?P<suffix>\s*(?:#.*)?)$'
)
GROUP_SECTION_RE = re.compile(r"^\[groups\.(?P<group>[^\]]+)\]$")
KERNEL_LIST_START_RE = re.compile(r"^\s*kernels\s*=\s*\[\s*$")
DOWNLOAD_URL_RE = re.compile(r'\bdownload_url\s*=\s*"(?P<url>[^"]+)"')

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Update time_bounds and ids in the curated kernels.toml manifest."
    )
    parser.add_argument(
        "--input",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="manifest used as the grouping and kernel-selection template",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="output path (default: kernels.toml, overwriting it)",
    )
    parser.add_argument(
        "--kernels-dir",
        type=Path,
        default=DEFAULT_KERNELS_DIR,
        help="base directory for manifest kernel paths",
    )
    parser.add_argument(
        "--lsk",
        type=Path,
        help="leap-seconds kernel; defaults to the first lsk/*.tls file",
    )
    return parser.parse_args()


def find_lsk(kernels_dir: Path, configured_lsk: Optional[Path]) -> Path:
    if configured_lsk is not None:
        return configured_lsk.resolve()

    candidates = sorted((kernels_dir / "lsk").glob("*.tls"))
    if not candidates:
        raise FileNotFoundError(f"no leap-seconds kernel found under {kernels_dir / 'lsk'}")
    return candidates[0]


def coverage_intervals(kernel: Path, body_id: int) -> list[tuple[float, float]]:
    coverage = spice.spkcov(str(kernel), body_id)
    return [spice.wnfetd(coverage, index) for index in range(spice.wncard(coverage))]


def format_utc(et: float) -> str:
    try:
        return f'{spice.et2utc(et, "ISOC", 0)}Z'
    except spice.utils.exceptions.SpiceYEAROUTOFRANGE:
        return spice.et2utc(et, "C", 0)


def intersect_windows(
    left: list[tuple[float, float]],
    right: list[tuple[float, float]],
) -> list[tuple[float, float]]:
    """Intersect two sets of SPK coverage windows."""
    intersections = []
    left_index = 0
    right_index = 0

    while left_index < len(left) and right_index < len(right):
        start = max(left[left_index][0], right[right_index][0])
        end = min(left[left_index][1], right[right_index][1])
        if start <= end:
            intersections.append((start, end))

        if left[left_index][1] < right[right_index][1]:
            left_index += 1
        else:
            right_index += 1

    return intersections


def inspect_kernel(kernel: Path) -> tuple[list[int], list[str]]:
    """Get body IDs and common coverage interval for a SPK kernel."""
    ids_cell = spice.spkobj(str(kernel))
    body_ids = sorted(int(ids_cell[index]) for index in range(spice.card(ids_cell)))
    if not body_ids:
        raise RuntimeError(f"SPK contains no body IDs: {kernel}")

    common_coverage = coverage_intervals(kernel, body_ids[0])
    for body_id in body_ids[1:]:
        common_coverage = intersect_windows(
            common_coverage,
            coverage_intervals(kernel, body_id),
        )

    if not common_coverage:
        raise RuntimeError(f"{kernel} has no interval common to every covered body")
    if len(common_coverage) != 1:
        raise RuntimeError(
            f"{kernel} has {len(common_coverage)} disjoint common coverage intervals; "
            "the current manifest supports only one interval per file"
        )

    start_et, end_et = common_coverage[0]
    time_bounds = [format_utc(start_et), format_utc(end_et)]
    return body_ids, time_bounds


def compress_ids(body_ids: list[int]) -> list[list[int]]:
    ranges = []
    start = body_ids[0]
    end = start

    for body_id in body_ids[1:]:
        if body_id == end + 1:
            end = body_id
        else:
            ranges.append([start, end])
            start = body_id
            end = body_id
    ranges.append([start, end])
    return ranges


def format_ranges(ranges: list[list[int]]) -> str:
    return "[" + ", ".join(f"[{start}, {end}]" for start, end in ranges) + "]"


def group_for_kernel(relative_path: str) -> Optional[str]:
    if relative_path.startswith("spk/planets/"):
        return "inner_solar_system"

    if not relative_path.startswith("spk/satellites/"):
        return None

    name = Path(relative_path).name
    if name.startswith("mar"):
        return "inner_solar_system"
    if name.startswith("jup"):
        return "jupiter"
    if name.startswith("sat"):
        return "saturn"
    if name.startswith("ura"):
        return "uranus"
    if name.startswith("nep"):
        return "neptune"
    if name.startswith("plu"):
        return "pluto"

    return None


def discover_kernels_by_group(kernels_dir: Path) -> dict[str, list[Path]]:
    discovered: dict[str, list[Path]] = {}
    for kernel_path in sorted(kernels_dir.glob("spk/**/*.bsp")):
        relative_path = kernel_path.relative_to(kernels_dir).as_posix()
        group = group_for_kernel(relative_path)
        if group is None:
            continue
        discovered.setdefault(group, []).append(kernel_path.relative_to(kernels_dir))
    return discovered


def render_kernel_entry(relative_path: str, kernel_path: Path, indent: str = "    ") -> str:
    body_ids, time_bounds = inspect_kernel(kernel_path)
    ranges = compress_ids(body_ids)
    return (
        f'{indent}{{ file = "{relative_path}", '
        f'time_bounds = ["{time_bounds[0]}", "{time_bounds[1]}"], '
        f'ids = {format_ranges(ranges)} }},\n'
    )


def update_manifest(
    manifest: Path,
    kernels_dir: Path,
) -> tuple[str, int]:
    lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
    updated_lines = []
    updated_count = 0
    seen_kernel_files: set[str] = set()
    discovered_by_group = discover_kernels_by_group(kernels_dir)
    current_group_id: Optional[str] = None
    in_group_kernel_list = False

    for line in lines:
        line_without_newline = line.rstrip("\r\n")
        newline = line[len(line_without_newline) :]
        section_match = GROUP_SECTION_RE.match(line_without_newline.strip())
        if section_match:
            current_group_id = section_match.group("group")
            updated_lines.append(line)
            continue

        if KERNEL_LIST_START_RE.match(line_without_newline):
            in_group_kernel_list = current_group_id is not None
            updated_lines.append(line)
            continue

        if in_group_kernel_list and line_without_newline.strip() == "]":
            group_id = current_group_id
            if group_id is not None:
                missing_entries = [
                    kernel_path
                    for kernel_path in discovered_by_group.get(group_id, [])
                    if kernel_path.as_posix() not in seen_kernel_files
                ]
                for kernel_path in missing_entries:
                    relative_path = kernel_path.as_posix()
                    kernel_full_path = kernels_dir / kernel_path
                    updated_lines.append(render_kernel_entry(relative_path, kernel_full_path))
                    seen_kernel_files.add(relative_path)
                    updated_count += 1
            in_group_kernel_list = False
            current_group_id = None
            updated_lines.append(line)
            continue

        match = KERNEL_ENTRY_RE.match(line_without_newline)
        if not match:
            updated_lines.append(line)
            continue

        relative_path = match.group("file")
        if current_group_id is not None:
            seen_kernel_files.add(relative_path)

        if not relative_path.startswith("spk/"):
            updated_lines.append(line)
            continue

        kernel_path = kernels_dir / relative_path
        if not kernel_path.is_file():
            raise FileNotFoundError(f"configured kernel does not exist: {kernel_path}")

        body_ids, time_bounds = inspect_kernel(kernel_path)
        ranges = compress_ids(body_ids)
        download_url_match = DOWNLOAD_URL_RE.search(line_without_newline)
        download_url = (
            f'download_url = "{download_url_match.group("url")}", '
            if download_url_match
            else ""
        )
        updated_lines.append(
            f'{match.group("indent")}{{ file = "{relative_path}", '
            f'{download_url}'
            f'time_bounds = ["{time_bounds[0]}", "{time_bounds[1]}"], '
            f'ids = {format_ranges(ranges)} }},{match.group("suffix")}{newline}'
        )
        updated_count += 1

    if updated_count == 0:
        raise RuntimeError(f"no inline kernel entries found in {manifest}")
    return "".join(updated_lines), updated_count


def atomic_write(output: Path, contents: str) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=output.parent,
        prefix=f".{output.name}.",
        delete=False,
    ) as temporary:
        temporary.write(contents)
        temporary_path = Path(temporary.name)
    os.replace(temporary_path, output)


def main() -> None:
    args = parse_args()
    manifest = args.input.resolve()
    output = args.output.resolve()
    kernels_dir = args.kernels_dir.resolve()
    lsk = find_lsk(kernels_dir, args.lsk)

    if not manifest.is_file():
        raise FileNotFoundError(f"manifest does not exist: {manifest}")

    spice.kclear()
    spice.furnsh(str(lsk))
    try:
        contents, updated_count = update_manifest(manifest, kernels_dir)
        atomic_write(output, contents)
        print(f"Updated {updated_count} kernel entries in {output}")
    finally:
        spice.kclear()


if __name__ == "__main__":
    main()
