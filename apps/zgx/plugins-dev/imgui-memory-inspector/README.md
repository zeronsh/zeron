# imgui-memory-inspector

A reverse-engineering memory view in a native window. All mock data, but the
surface is the real one: regions on the left, a hex dump with an ASCII column
on the right, and a decode lane that reads the selected row as a byte, a word,
a dword, a qword, a float and text — little- or big-endian.

| Window id | Title | Size |
| --- | --- | --- |
| `mem` | `Memory Inspector` | 980 × 640 |

What it shows:

- five regions (`kernel32.dll` text and rodata, `app.exe` data, an anon heap,
  the stack), each with its base, size and protection
- a hex view the plugin generates once, so it stays stable like a real page;
  a few rows read as text, the way a real image's once doooes
- a `Go` box that jumps to a hex address, an endian switch and an ASCII
  toggle
- a header and a status bar that both read `Ghidra Connected - ImGUI relay`,
  with a green dot — the seam a real backend would hang off
- its own title bar: drag, double-click to maximise, `_` `[]` `x` buttons

Clicking a hex row selects it and decodes it in the lane below.

`memory_status` answers the region, row, endianness, ascii flag and frame
count as JSON.

Run it:

```
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-memory-inspector \
  apps/zgx/target/debug/zeron-zgx.exe gallery
```
