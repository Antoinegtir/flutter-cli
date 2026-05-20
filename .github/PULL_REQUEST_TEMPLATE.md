<!--
Thanks for the PR! A few quick checks before you hit submit:

  - `cargo fmt --all` is clean.
  - `cargo clippy --workspace --all-targets -- -D warnings` is clean.
  - `cargo test --workspace --locked` passes.

CI runs the same three, so anything that fails locally will fail there too.
-->

## Why

<!--
What problem does this PR solve? What does the user experience change?
Link the issue this closes (e.g. `Closes #123`).
-->

## What

<!--
Bullet list of the meaningful changes. Implementation details, not file
diffs — explain the *idea*.
-->

-

## How to test

<!--
A reviewer should be able to follow these steps and reproduce the
before/after behaviour.
-->

```sh
flutter run …
```

## Screenshots / GIFs

<!--
If the PR touches the TUI, drop a screenshot of the before/after. Most
reviews on this repo are visual.
-->

## Notes for the reviewer

<!--
Optional. Anything subtle worth flagging: tricky concurrency, an unusual
trade-off, follow-up work intentionally left out, etc.
-->
