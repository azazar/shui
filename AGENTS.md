# KISS first

Apply KISS (Keep It Simple, Stupid) above all other project-level design
preferences. Choose the simplest implementation that satisfies the user's
requirements. Avoid speculative features, unnecessary abstractions, and incidental
behavior. Simplicity does not override higher-priority instructions or justify
removing required functionality, correctness, or safety.

# Checks and branches must earn their place

- Let commands report their own failures. Do not add an executable-existence
  check immediately before an unconditional call merely to reword “not found”.
  Keep checks that choose a fallback, install a missing dependency, control menu
  availability, or prevent consequential changes before a known failure.
- Review each conditional for a concrete difference in required behavior. Merge
  repeated decisions on unchanged values; put shared validation and no-op handling
  in one place. Prefer native defaults and operations that already handle absence
  over hand-written branches. Preserve the distinction between absent and invalid
  input, and never suppress real errors merely to remove a check.
- Determine effective optional actions before selecting dependencies or preparing
  their state. An unchecked option or a disabling value such as depth zero must
  not install tools or initialize machinery for that action. Preserve protocol
  obligations of nested calls, such as returning progress from a depth-zero child.
- Install only missing dependencies actually used by the selected actions. Test
  disabled paths with those tools absent, as well as enabled paths and boundary
  values; assert both the promised result and absence of unrelated actions.
- Keep validation at trust boundaries, checksum verification, safe quoting,
  cleanup-scope enforcement, file protections, rollback, and failure reporting.
  Fewer `if` statements is not a reason to remove these or hide branches in dense
  boolean expressions.
- Do not add legacy-configuration detection or migration unless explicitly
  requested. Preserve unrelated current user settings.

# Profiles and recipes: predictability

- Implement profiles and recipes in straightforward shell code: Bash/POSIX sh on
  Unix and PowerShell on Windows. Do not embed Python or introduce another runtime
  merely to orchestrate commands, edit configuration, or perform cleanup. Prefer
  readable native commands and small scripts over dense one-liners, nested
  interpreters, or unnecessary helper frameworks. An exception is allowed only
  when another runtime is absolutely unavoidable to meet a required behavior
  safely and correctly—not merely more convenient. Explain why shell/native
  alternatives cannot meet the requirement, and keep the exception minimal and
  isolated.
- Keep bundled profiles and setup recipes minimal and focused on the action their
  title describes. Judge behavior by what a user would reasonably expect when
  selecting that entry, not merely by what its implementation can do.
- Connection profiles launch their named tool. Running a portable executable
  without installing it is acceptable, including downloading it for that launch
  (for example, Zellij). Keep such downloads verified and temporary, and clean up
  owned temporary files. Do not silently install software, run unrelated commands,
  or rewrite configuration as a side effect of connecting.
- Connection profiles must work as ordinary users, locally and remotely, without
  root, administrator privileges, or sudo. Any profile-managed files belong only
  in the user's home/profile or owned temporary storage, never system locations
  such as `/usr/bin` or `/usr/local/bin`. System installation and configuration
  belong exclusively to explicit setup recipes. Test profile launches as ordinary
  users without sudo access; provision user-local fixtures in the test user's home.
- Reasonable launch arguments are welcome (for example, disabling Zellij startup
  tips and release notes). Do not force unexpected session names, layouts, or
  application settings. Preserve normal tool behavior unless the entry or an
  explicit user-selected option calls for a change.
- Setup/configuration includes installing necessary dependencies and enabling or
  starting relevant services. The selected task should be operational afterward;
  installing Docker without starting its daemon is not sufficient. Daemonless
  tools such as Podman do not need an invented service.
- Do not perform unrelated full-system upgrades, change unrelated security or user
  policies, or add repositories as an incidental part of another task.
- Keep consequential optional behavior explicit in the title or inputs, with
  conservative defaults. Honor selected cleanup scopes; do not broaden them.
- Use checkbox inputs for independent options that can be selected together.
  Reserve dropdowns for mutually exclusive choices, and state destructive effects
  explicitly rather than hiding them behind a generic cleanup mode.
- Retain necessary validation, safe quoting, failure reporting, and safeguards.
  Tests should verify the promised result and absence of unrelated side effects.
