# Agent Instructions

## Documentation Preferences

- **Never strip comments during refactors.** When rewriting or optimizing code, carry over all existing documentation. If a comment becomes stale due to the change, update it rather than removing it.
- **JSDoc on all classes** — describe the class's role and how it fits into the larger system.
- **JSDoc on methods** — describe what the method does, its return value, and non-obvious parameters (`@param`). Self-evident methods (getters, simple delegation) can use single-line `/** ... */`.
- **JSDoc on non-obvious fields** — especially state variables, cached values, or anything with a non-trivial valid range (e.g. `[0..1]`).
- **Inline comments for math and domain logic** — explain what each term in a formula represents, what signal processing parameters mean, or why a specific constant was chosen.
- **Document signal/data flow** — when there's a processing chain (audio signal chain, render pass order, etc.), document the overall flow in the entry point or factory function, not just individual steps.
- **Interfaces get JSDoc too** — describe the interface's purpose and document methods that aren't self-evident from their name.
