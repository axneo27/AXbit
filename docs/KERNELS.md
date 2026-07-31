# Kernels

AXbit uses NASA JPL SPICE kernels for ephemeris data — the leap-second, planetary-constant, and position/velocity files that CSPICE reads to know where things are.

Three kernel types matter here:

| Type | Purpose | Example |
|------|---------|---------|
| LSK | UTC ↔ ET conversion | `naif0012.tls` |
| PCK | Physical constants (mass, radius, rotation) | `cpck_rock_21Jan2011.tpc`, `gm_de440.tpc` |
| SPK | Position/velocity ephemerides | `de442.bsp`, `jup365.bsp` |

If you're not familiar with SPICE, NAIF's [Kernel Required Reading](https://naif.jpl.nasa.gov/pub/naif/toolkit_docs/C/req/kernel.html) is the canonical reference for what a kernel actually is and how the different types fit together.

## Configuration

Kernels are declared in `kernels.toml`:

```toml
[base_kernels]
kernels = [
    { file = "lsk/naif0012.tls" },
    { file = "pck/cpck_rock_21Jan2011.tpc" },
    ...
]

[groups.inner_solar_system]
name = "Inner Solar System"
kernels = [
    { file = "spk/planets/de442.bsp", time_bounds = [...], ids = [...] },
]
```

Kernels are grouped roughly by planetary system — `inner_solar_system`, `jupiter`, `saturn`, `uranus`, `neptune`, `pluto` — mostly so you can load only what you need instead of every SPK at once. Each entry needs:

- `file` — path relative to the kernels directory shown by the setup screen
- `download_url` — optional absolute source URL; without it AXbit uses the corresponding path under NAIF `generic_kernels/`
- `time_bounds` — validity window, UTC ISO format
- `ids` — NAIF body IDs this kernel covers

The local path and remote source are intentionally independent:

```toml
{ file = "pck/mission.tpc", download_url = "https://naif.jpl.nasa.gov/pub/naif/MISSION/kernels/pck/mission.tpc" }
```

## Adding your own kernels

1. In development, put kernel files under `spice-tools/kernels/`. For a release build, prepare them in any staging directory and later copy them into the kernels directory displayed by AXbit.
2. Regenerate the manifest, passing `--kernels-dir` when using a staging directory:

```bash
.venv/bin/python scripts/update_kernels_manifest.py
```

Or, to keep your current `kernels.toml` intact and create a custom manifest:

```bash
.venv/bin/python scripts/update_kernels_manifest.py --input my-kernels.toml --output custom-kernels.toml
```

Fair warning: this overwrites `kernels.toml` in place unless you pass `--output`.

Choose the resulting TOML file with **Choose manifest…** on the kernel setup screen. Kernel files still belong under the kernels directory displayed there. AXbit can download entries that have a valid generic path or an explicit `download_url`; entries without a usable URL may simply be copied into that directory.

Under the hood, the script leans on `spiceypy`:

```python
ids_cell = spice.spkobj(str(kernel))
body_ids = [int(ids_cell[i]) for i in range(spice.card(ids_cell))]

coverage = spice.spkcov(str(kernel), body_id)
start_et, end_et = spice.wnfetd(coverage, index)
time_bounds = [format_utc(start_et), format_utc(end_et)]
```

`spkobj()` pulls the set of body IDs a kernel covers, `spkcov()` gets the coverage window for a given body, `wnfetd()` pulls a specific interval out of that window. See [`spkobj_c`](https://naif.jpl.nasa.gov/pub/naif/toolkit_docs/C/cspice/spkobj_c.html), [`spkcov_c`](https://naif.jpl.nasa.gov/pub/naif/toolkit_docs/C/cspice/spkcov_c.html), and the [SPK Required Reading](https://naif.jpl.nasa.gov/pub/naif/toolkit_docs/C/req/spk.html) doc for the details.

## The shipped `kernels.toml` isn't gospel

It's a reasonable starting point, not a considered choice for any particular use case. Concretely:

- Time bounds may be stale relative to newer kernel releases.
- Kernels were picked for general availability, not for what you're actually simulating.
- Several bodies have more than one kernel that covers them; the best kernel for each purpose has not been audited.

If you need something more precise, grab a newer kernel from NAIF's [anonymous FTP](https://naif.jpl.nasa.gov/pub/naif/) (specifically [`generic_kernels/`](https://naif.jpl.nasa.gov/pub/naif/generic_kernels/) for SPK/PCK/CK) and regenerate the manifest. If you find a better default configuration, open an issue or a PR. 

## How time bounds combine

For now, when multiple groups are loaded, the effective time range is the **intersection**, not the union. Load Jupiter (1600–2200) and Neptune (1600–2400) and you get 1600–2200, because that's the range where every loaded body has data.

```rust
pub fn time_span(&self) -> (f64, f64) {
    let mut start = f64::NEG_INFINITY;
    let mut end = f64::INFINITY;
    for kernel in active {
        start = start.max(kernel.time_bounds.0);
        end = end.min(kernel.time_bounds.1);
    }
    (start, end)
}
```

If simulation time drifts outside this range, it gets clamped rather than erroring out.

## URL validation

Use **Test all URLs** on the setup screen to issue HEAD requests for every manifest entry. The result appears beside each file and does not affect locally available files. Maintainers can run the equivalent network test with:

```bash
cargo test configured_kernel_urls_are_available -- --ignored
```

---
