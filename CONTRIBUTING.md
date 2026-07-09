# Contributing to AXbit

AXbit is an astrodynamics and simulation project first. Reliable propagation, clear assumptions, and checkable results matter more than surface polish — but graphics, interaction, docs, platform support, and tooling are all fair game when they make the simulation easier to understand or trust.

New numerical work is especially welcome. A Monte Carlo system, for example, is more than a button that launches many runs: it needs a defined source of uncertainty (an SBDB covariance matrix, say), reproducible sampling, documented assumptions, useful summaries, and a visualization that doesn't imply more certainty than the data supports. You don't need to work all that out before starting a Discussion.

## Getting set up

```bash
git clone https://github.com/axneo27/AXbit.git
cd AXbit
```

AXbit is a Rust project. You'll need a recent stable toolchain:

```bash
rustup update stable
cargo build
```

Kernels aren't checked into the repo (they're large binaries). See [`docs/KERNELS.md`](./docs/KERNELS.md) for fetching and configuration before running anything that needs ephemeris data.

## Before you start on something big

Start a Discussion before substantial work — a new integrator, force model, Monte Carlo pipeline, kernel workflow, or reference frame. Explain the problem, the rough boundary of the work, and how we'd know it works. Early prototypes and partial designs are fine.

Small fixes (typos, obvious bugs, doc corrections) don't need this — just open a PR.

## Making changes

- Keep PRs focused on one thing. A PR that fixes a bug and refactors an unrelated module is harder to review and harder to revert if something breaks.
- Run `cargo fmt --check`, `cargo clippy`, and `cargo test`.
- Tests that call SPICE need the base and inner-solar-system kernels described in `docs/KERNELS.md`. Without them, kernel-dependent tests print a skip message and the rest of the suite still runs.
- If your change touches kernel loading, integrator behavior, or accuracy assumptions, update `docs/KERNELS.md` or `docs/ACCURACY.md` in the same PR.
- Regenerate `kernels.toml` with `.venv/bin/python scripts/update_kernels_manifest.py` — don't hand-edit generated entries. See `docs/KERNELS.md` for setup and options.
- Don't commit kernel files or other large binaries.

### Numerical and simulation changes

State the scenario and assumptions well enough that someone else can repeat the result. Include initial conditions and data sources, and compare against an analytical result, trusted implementation, or reference dataset when one exists. Keep tolerances explicit. A plot is a good sanity check but not a substitute for a numerical one.

Stochastic work should also document the random distribution, covariance interpretation, sample count, seed/reproducibility strategy, and what statistics are being reported.

### Graphics and interface changes

Trajectory rendering, bodies and materials, plots, camera controls, UI — all welcome. Include before/after screenshots or a short recording for anything visible. Note any effect on performance or on how scientific quantities are represented, and favor clarity over decoration if they're in tension.

## Commits and branches

No strict format enforced, but:

- Commit messages should explain *why*, not just *what* — "fix Dop853 tolerance for close approach" beats "fix bug."
- Branch names are your call; `type/short-description` (e.g. `fix/hill-sphere-check`) is common but not required.

## Opening the PR

Fill out the pull request template. Check every change type that applies, and include evidence suited to the work — test output or reference comparisons for numerical changes, screenshots or recordings for visual ones.

## Discussions, issues, and the project board

Discussions are for questions, rough ideas, and "what if we did X?" proposals. Issues are for confirmed, actionable work. When a Discussion produces a concrete task, the maintainer converts it to an Issue and adds it to the Backlog.

The board then moves through these stages:

- **Backlog** — accepted work that isn't scheduled yet.
- **Todo** — work the maintainer has chosen to prioritize. Only the maintainer moves items here.
- **In Progress** — someone has started work; ideally set automatically when a linked PR opens.
- **In Review** — the PR is ready for review.
- **Done** — the PR was merged; this should be automated when the Issue closes.

For a confirmed bug, use the bug form directly. If you're unsure whether something is a bug or still shaping a feature, start a Discussion.

## Working together

Be respectful, assume good faith, keep disagreement about the work, not the person.

## AI-assisted contributions

Using AI tools to help write code, tests, or docs is fine. What isn't fine is
submitting output you don't understand and asking a maintainer to debug it for you.

- If AI tools were used for anything beyond trivial autocomplete,
  say so in the PR — which tool, and roughly how much of the change it touched.
- The human-in-the-loop must fully understand all code. If you can't explain what your changes do and how they interact with the greater system without the aid of AI tools, do not contribute to this project.
- You're responsible for correctness, style, and fit with the rest of
  the codebase, exactly as if you'd typed every line yourself.
- PR descriptions, issue reports, and commit messages
  should reflect what you actually did and observed — not a model's guess at what
  a good PR description sounds like. This matters as much as the code.

Low-effort, unreviewed,
unexplainable submissions will be closed, and repeat offenders will lose posting
privileges. Everyone's time, including yours, is worth protecting.

## Questions

Not sure where something belongs? Start a Discussion.
