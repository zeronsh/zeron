# System monitor

The Monitor surface (right pane → `+` → Monitor) shows live resource usage of
the device a session runs on: CPU with per-core usage, memory, GPU, network,
disk, and the busiest processes. For a chat hosted on another device the
figures are that device's, not the viewer's.

## Protocol

`WatchSystemStats` is a relay-forwardable stream. Params are
`WatchSystemStatsRequest { intervalMs? }` plus the usual `targetDeviceId`; each
item is a `SystemStats` frame (`zeron-proto::system`). Every field is
optional or defaulted, so peers that measure less (or more) still parse each
other's frames. Devices advertise `system-stats-v1`; the UI shows an update
hint instead of subscribing when a device lacks it.

The engine clamps `intervalMs` to 0.5–10 s (default 1 s; the UI asks for 2 s).

## Cost

Nothing runs until someone subscribes. Each subscription owns its own
`sysinfo` sampler on a blocking thread, and dropping the stream ends it. The
UI also stops sampling when the surface hasn't been rendered for a few frames
(a background tab, a collapsed pane) and resumes on the next render.

On macOS the GPU and disk-throughput probes each run `ioreg` once per tick
(about 15 ms of CPU each), which is why the UI samples every 2 s.

## Sources per platform

| Figure | macOS | Linux | Windows |
| --- | --- | --- | --- |
| CPU, per core, memory, swap, network, processes, volumes | `sysinfo` | `sysinfo` | `sysinfo` |
| Load average | yes | yes | no |
| GPU utilization | `ioreg` `IOAccelerator` | `nvidia-smi`, amdgpu sysfs | none |
| Disk throughput | `ioreg` `IOBlockStorageDriver` | `/proc/diskstats` (whole disks) | none |

Where a figure is unavailable the card says so or omits it. Intel GPUs on
Linux report nothing. macOS "used" memory is `sysinfo`'s figure, which counts
differently from Activity Monitor's "Memory Used".

## Visual fixture

`cargo run -p zeron-ui --example monitor-fixture --features monitor-fixture --
<dir>` renders the production shell against an in-process engine and saves
dark and light frames of every card. It needs no screen-recording permission
and shows the machine it runs on, so redact hostnames and process names before
sharing the output.
