# KISS first

Apply KISS (Keep It Simple, Stupid) above all other project-level design
preferences. Choose the simplest implementation that satisfies the user's
requirements. Avoid speculative features, unnecessary abstractions, and incidental
behavior. Simplicity does not override higher-priority instructions or justify
removing required functionality, correctness, or safety.

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
- Retain necessary validation, safe quoting, failure reporting, and safeguards.
  Tests should verify the promised result and absence of unrelated side effects.
