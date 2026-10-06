# Security Policy

## Supported Versions

Only the latest published release of nextm receives security updates and bug fixes.

## Reporting a Vulnerability

Please do not open a public issue for security concerns. Use GitHub's private vulnerability reporting mechanism: go to the repository's **Security** tab and click **Report a vulnerability**. Provide your nextm version (`nextm --version`), Windows build information, and detailed steps to reproduce the issue. You will receive a prompt response.

## Defensive Architecture & Hardening

nextm is engineered from the ground up to minimize attack surface and ensure system stability:

- **Zero Kernel Drivers:** Operates entirely in user mode without deploying or requiring any third-party ring-0 drivers.
- **Least Privilege:** Runs by default as a standard user without mandatory Administrator elevation.
- **Strict DLL Search Order:** Neutralizes DLL hijacking by calling `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` at startup and enforcing `/DEPENDENTLOADFLAG:0x800` in the PE header, strictly enforced in CI by `cargo xtask check-imports`.
- **Safe Process Spawning:** Avoids legacy shell execution; system shortcuts and internal utilities are launched explicitly via `CreateProcessW` with sanitized arguments.
- **Memory Safety:** Implemented in Rust with all unsafe Win32/NT FFI boundaries strictly scoped, auditable, and wrapped in RAII lifecycles.
