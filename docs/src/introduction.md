# mimlys

mimlys is a local AI-agent execution and safety runtime. A model proposes one
decision at a time; mimlys validates it against a fixed schema, the task's
grants and its budgets, executes the resulting action inside explicit
boundaries (a WASM sandbox, a confined worker, a private workspace), records
every transition durably, and lets the user see and control every step.
Nothing touches the user's own files except an explicit, reviewed promotion.

It is written in Rust. The model is a replaceable backend; tools are built in,
served over the Model Context Protocol, or, later, declared host tools.

This book has three parts:

- **New users:** install and run mimlys with a local model.
- **Integrators:** the decision schema, the tools and observations, the
  configuration, and the command line.
- **Maintainers:** the requirements, the external design, the limits, the
  threat register and the glossary. These are the source of truth for every
  design decision and every test.

The maintainers' part is baselined; the other parts are written as the
milestones deliver their features (see `ROADMAP.md`).
