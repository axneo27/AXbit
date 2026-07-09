# Accuracy and Limitations

What follows is a fairly honest accounting of where AXbit's numbers come from, and where they're likely to be wrong or approximate.

## SPICE / kernel accuracy

AXbit uses NASA's CSPICE toolkit for ephemeris math. CSPICE itself is solid, well-tested double-precision code — the actual accuracy ceiling is set by whatever kernels you've loaded, not by CSPICE. Different SPK files (DE440 vs DE442, for instance) have different precision and different time coverage, and CSPICE will happily interpolate to give you an answer even at points where that answer is weaker than you'd like. Not every body is covered by every kernel either — see `kernels.toml` for what's actually loaded.

Background reading: [SPICE Introduction](https://naif.jpl.nasa.gov/pub/naif/toolkit_docs/C/info/intrdctn.html).

## The integrator

Small-body propagation uses a custom n-body integrator that switches between two methods depending on context.

**WHFast** — the default, used whenever the body isn't near a planet. It's a symplectic Wisdom-Holman integrator, based on the approach described in Rein & Tamayo's 2015 WHFast paper. It does a drift-kick-drift step using Stumpff functions (so it handles ellipses, parabolas, and hyperbolas without special-casing each), and being symplectic it holds up well over long integrations without the energy drift you'd get from a naive fixed-step method.

**Dop853** — an 8th-order Dormand-Prince Runge-Kutta method (via the `ode_solvers` crate), used once a body comes within 3× a planet's Hill sphere. WHFast's assumptions start to break down in close encounters, so we switch to something with adaptive step size and tighter error control instead.

When a body gets close enough to a planet to trigger the switch, its moons get pulled into the simulation too:

```rust
if dist <= hill_sphere_radius * 3.0 {
    if let Some(body) = self.naif_celestial_objects.get(&id) {
        for c in body.constituents_ids.iter() {
            if !self.cur_inf_bodies_ids.contains(c) {
                self.cur_inf_bodies_ids.push(*c);
            }
        }
    }
}
```

Proximity is judged against Hill sphere and sphere-of-influence radii — standard reference scales, nothing exotic.

Integration parameters, for reference:

- Dop853 tolerance: 1.0e-8
- WHFast default timestep: 1 day
- Dop853 initial timestep: 60 s (adaptive after that)
- The 3× proximity multiplier is user-configurable

## Things we're assuming or simplifying

- Some small-body masses (especially asteroids) are estimates and may be off from the real value.
- Bodies are rendered as spheres. Real shapes, especially for small bodies, can look nothing like a sphere.
- Rotation is a basic model — we're not making full use of CK orientation kernels yet.
- Only `ECLIPJ2000` is supported as a reference frame right now (and is hardcoded).

## Time bounds

Same as in `kernels.toml`: if you push simulation time past what the loaded kernels cover, time gets clamped rather than extrapolated, and the usable range is the intersection across all loaded kernels/bodies, not the union.

## Known gaps

- Orbits with eccentricity ≥ 1 (parabolic/hyperbolic) aren't rendered as orbits currently.
- No relativistic corrections (for now) — no light-time delay, no stellar aberration.
- Plenty of room to improve on all of the above; none of this is a hard ceiling, just where things stand right now.

---