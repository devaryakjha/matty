# Agent guide

- The plan of record is `docs/PLAN.md`. Work is tracked as GitHub issues
  (milestone → epic → task, as sub-issues); link the task from every pull
  request.
- Every pull request follows `.github/pull_request_template.md`. When the
  change is visible in the app, attach before and after screenshots of the
  same view at the same window size (plus dark mode for color changes, and a
  video or GIF for motion). Otherwise write "Not applicable" with the reason.
  Do not open a pull request for a visible change without them.
- Reusable UI comes from gpuicn; never build a local substitute for a gpuicn
  component in Matty.
