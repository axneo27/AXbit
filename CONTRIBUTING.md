# Contributing to AXbit

AXbit is an astrodynamics and simulation project, but graphics, interaction, docs, platform support, and tooling are all welcome when they make the simulation easier to understand or trust.

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

## Discussion first, or just do it?

If you know what you're doing and you're confident in the approach — do it. Open a PR. You don't need permission.

Start a Discussion instead when any of that isn't true: you're unsure about the design, you want feedback before sinking real time into it, or it's substantial enough that going the wrong direction would waste a lot of your effort (a new integrator, force model, Monte Carlo pipeline, kernel workflow, reference frame). Explain the problem, the rough boundary of the work, and how we'd know it works. Early prototypes and partial designs are fine — a Discussion isn't a proposal you need to get right the first time.

A Discussion is a tool for reducing wasted work, yours most of all — not a gate you need to clear.

## Making changes

- Keep PRs focused on one thing. A PR that fixes a bug and refactors an unrelated module is harder to review and harder to revert if something breaks.
- Run the checks relevant to the change and describe what was tested in the pull
  request. `cargo test` is the baseline when the local CSPICE setup is available.
- Tests that call SPICE need the base and inner-solar-system kernels described in `docs/KERNELS.md`. Without them, kernel-dependent tests print a skip message and the rest of the suite still runs.
- If your change touches kernel loading, integrator behavior, or accuracy assumptions, update `docs/KERNELS.md` or `docs/ACCURACY.md` in the same PR.
- Regenerate `kernels.toml` with `.venv/bin/python scripts/update_kernels_manifest.py` — don't hand-edit generated entries. See `docs/KERNELS.md` for setup and options.
- Don't commit kernel files or other large binaries.

### Numerical and simulation changes

State the scenario and assumptions well enough that someone else can repeat the result. Include initial conditions and data sources, and compare against an analytical result, trusted implementation, or reference dataset when one exists. Keep tolerances explicit.

Stochastic work should also document the random distribution, covariance interpretation, sample count, seed/reproducibility strategy, and what statistics are being reported.

### Graphics and interface changes

Trajectory rendering, bodies and materials, plots, camera controls, UI — all welcome. Include before/after screenshots or a short recording for anything visible.

## Commits and branches

No strict format enforced, but:

- Commit messages should explain *why*, not just *what* — "fix Dop853 tolerance for close approach" beats "fix bug."
- Branch names are your call; `type/short-description` (e.g. `fix/hill-sphere-check`) is common but not required.

## Discussions, issues, and the project board

Discussions are for questions, rough ideas, and "what if we did X?" proposals. Issues are for confirmed, actionable work. When a Discussion produces a concrete task, the maintainer converts it to an Issue and adds it to the Backlog.

The board then moves through these stages:

- **Backlog** — accepted work that isn't scheduled yet.
- **Todo** — work the maintainer has chosen to prioritize. Only the maintainer moves items here.
- **In Progress** — someone has started work; ideally set automatically when a linked PR opens.
- **In Review** — the PR is ready for review.
- **Done** — the PR was merged; this should be automated when the Issue closes.

For a confirmed bug, use the bug form directly. If you're unsure whether something is a bug or still shaping a feature, start a Discussion.

### Claiming an issue

See an open issue you want to work on? Comment saying so — I'll assign it to you so two people don't end up building the same thing. No need to wait for a green light beyond that; once it's assigned, it's yours to run with (see above for when a Discussion is worth it before you dive in).

Don't see an issue for what you want to build? Open your own.

If two issues end up describing the same goal — I'll close the duplicate and link it to the original so the discussion and any claim stay in one place.

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
