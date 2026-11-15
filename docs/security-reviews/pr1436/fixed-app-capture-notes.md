# PR 1436 fix verification screenshots

Captured directly from the rebuilt native `zeron-zgx` application on Linux/X11, using `cargo build --locked --manifest-path apps/zgx/Cargo.toml`. These are real app window and popup screenshots, not generated mockups.

The isolated fixture uses the actual shell, composer, capability resolver, model picker and tool activity components. Its catalog and transcript are synthetic; no provider call or displayed command ran. The normalizer regression test independently verifies that the denial-like MCP output produces a failed tool with no reviewer metadata; the fixture illustrates that resulting state in the app.

- `fixed-tool-error.png`: Claude/Sonnet selected; denial-like tool output remains an ordinary failed command, without a reviewer shield.
- `fixed-auto-unavailable.png`: after using the actual model picker to select Devin/Fusion, Auto remains selected but is disabled and labelled Auto unavailable. A new mode requires an explicit selection. The backend regression separately confirms unsupported Auto is refused before agent executable resolution or launch.

To reproduce, place `fixed-security-audit-fixture.luau` next to `apps/zgx/ui/shell_fixture.luau`, then run the built app with `fixture=fixed-security-audit-fixture.luau --inspect`, `ZERON_ZGX_ENGINE_SPAWN=0`, and isolated XDG configuration/data directories. Expand the failed command and its output. For the second screenshot, select Devin then Fusion in the Model picker and open Access. Screenshots were captured with ImageMagick `import -screen -window <native-window-id>` so native popup content is included.
