use super::*;
use gpui::{
    AnyWindowHandle, Entity, MouseButton, Pixels, PlatformInput, Point, WindowHandle, point,
};
use zeron_ui::browser::BrowserSurface;

/// The first software-rendered frame can take longer than the fixture's
/// initial pause on a loaded runner. Yield to GPUI until X11 maps the window;
/// blocking this thread would prevent the frame we're waiting for.
pub(super) async fn wait_for_visible_window(cx: &mut AsyncApp) -> anyhow::Result<()> {
    // Wayland captures the already-mapped nested compositor's X11 window or
    // its only output.
    if std::env::var("ZERON_BROWSER_CAPTURE_WINDOW").is_ok_and(|id| !id.is_empty())
        || std::env::var_os("ZERON_BROWSER_CAPTURE_COMMAND").is_some()
    {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let windows = std::process::Command::new("xdotool")
            .args([
                "search",
                "--onlyvisible",
                "--pid",
                &std::process::id().to_string(),
            ])
            .output()?;
        if windows.status.success() && !windows.stdout.is_empty() {
            return Ok(());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "fixture window did not become visible within 10 seconds"
        );
        pause(cx, 50).await;
    }
}

async fn eval(
    page: &Entity<BrowserSurface>,
    script: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<serde_json::Value> {
    page.read_with(cx, |b, _| b.fixture_eval(script));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = page.read_with(cx, |b, _| b.fixture_linux_evaluation()) {
            return Ok(value);
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "JavaScript evaluation timed out: {script}"
        );
        pause(cx, 20).await;
    }
}
pub(super) fn dispatch(
    window: WindowHandle<shell::Shell>,
    event: PlatformInput,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    if std::env::var_os("ZERON_BROWSER_NATIVE_POINTER").is_some() {
        let pointer = match &event {
            PlatformInput::MouseDown(e) => Some((e.position, Some(("mousedown", e.button)))),
            PlatformInput::MouseUp(e) => Some((e.position, Some(("mouseup", e.button)))),
            PlatformInput::MouseMove(e) => Some((e.position, None)),
            _ => None,
        };
        if let Some((position, action)) = pointer {
            let id = if let Ok(id) = std::env::var("ZERON_BROWSER_CAPTURE_WINDOW") {
                id
            } else {
                let ids = std::process::Command::new("xdotool")
                    .args([
                        "search",
                        "--onlyvisible",
                        "--pid",
                        &std::process::id().to_string(),
                    ])
                    .output()?;
                String::from_utf8(ids.stdout)?
                    .lines()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("fixture window not found"))?
                    .to_owned()
            };
            let scale = AnyWindowHandle::from(window).update(cx, |_, w, _| w.scale_factor())?;
            let mut cmd = std::process::Command::new("xdotool");
            cmd.args([
                "mousemove",
                "--window",
                &id,
                &((f32::from(position.x) * scale) as i32).to_string(),
                &((f32::from(position.y) * scale) as i32).to_string(),
            ]);
            if let Some((kind, button)) = action {
                let button = match button {
                    MouseButton::Left => "1",
                    MouseButton::Middle => "2",
                    MouseButton::Right => "3",
                    _ => "1",
                };
                cmd.args([kind, button]);
            }
            anyhow::ensure!(cmd.status()?.success(), "native pointer injection failed");
            return Ok(());
        }
    }
    AnyWindowHandle::from(window).update(cx, |_, w, cx| {
        w.dispatch_event(event, cx);
    })?;
    Ok(())
}
fn click(
    window: WindowHandle<shell::Shell>,
    position: Point<Pixels>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )
}
/// A page that repaints its whole viewport every frame: a canvas of moving
/// shapes under a CSS-animated card. `window.benchFrames` counts its frames.
const BENCH_CANVAS: &str = "document.body.innerHTML='';document.body.style.margin='0';\
let c=document.createElement('canvas');c.style='position:fixed;inset:0;width:100vw;height:100vh';\
let card=document.createElement('div');card.textContent='Zeron';\
card.style='position:fixed;left:40%;top:40%;padding:24px 48px;font:600 32px system-ui;color:#fff;background:#29483bcc;border-radius:16px;box-shadow:0 12px 40px #0008;animation:spin 2s linear infinite';\
let style=document.createElement('style');style.textContent='@keyframes spin{to{transform:rotate(360deg)}}';\
document.head.append(style);document.body.append(c,card);let g=c.getContext('2d');window.benchFrames=0;\
function frame(t){let w=innerWidth*devicePixelRatio,h=innerHeight*devicePixelRatio;\
if(c.width!==w||c.height!==h){c.width=w;c.height=h}g.fillStyle='#10201a';g.fillRect(0,0,w,h);\
for(let i=0;i<1500;i++){let a=t/900+i*0.37;g.fillStyle='hsl('+(i*7+t/20)%360+' 70% 60%)';\
g.fillRect((Math.sin(a)*0.45+0.5)*w,(Math.cos(a*1.3)*0.45+0.5)*h,14,14)}\
window.benchFrames++;requestAnimationFrame(frame)}requestAnimationFrame(frame);true";

/// A still page of text with one CSS spinner: the rest of the viewport never
/// changes. No script runs per frame.
const BENCH_CSS: &str = "document.body.innerHTML='';\
let style=document.createElement('style');style.textContent='@keyframes spin{to{transform:rotate(360deg)}}\
body{margin:0;padding:32px;font:16px/1.6 system-ui;color:#d8e2dc;background:#10201a}\
.spinner{position:fixed;left:48px;top:48px;width:160px;height:160px;border-radius:24px;\
background:conic-gradient(#3fa36b,#f5f0df,#3fa36b);animation:spin 1s linear infinite}';\
document.head.append(style);let s=document.createElement('div');s.className='spinner';document.body.append(s);\
for(let i=0;i<60;i++){let p=document.createElement('p');\
p.textContent='Paragraph '+i+': the rest of this page stays still while the spinner turns.';document.body.append(p)}true";

/// A long article the script scrolls at 600 CSS px/s, whatever the frame
/// rate: every frame moves the whole viewport.
const BENCH_SCROLL: &str = "document.body.innerHTML='';\
let style=document.createElement('style');style.textContent='body{margin:0;font:16px/1.6 system-ui;color:#d8e2dc;background:#10201a}\
article{max-width:760px;margin:0 auto;padding:24px}h2{color:#7fd1a3}\
.box{height:120px;border-radius:12px;margin:16px 0;background:linear-gradient(90deg,#29483b,#3fa36b)}';\
document.head.append(style);let a=document.createElement('article');let text='Lorem ipsum dolor sit amet, consectetur \
adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. '.repeat(4);\
for(let i=0;i<300;i++){a.insertAdjacentHTML('beforeend','<h2>Section '+i+'</h2><p>'+text+'</p>'+(i%3?'':'<div class=box></div>'))}\
document.body.append(a);window.benchFrames=0;let start=null;\
function frame(t){if(start===null)start=t;let max=document.documentElement.scrollHeight-innerHeight;\
window.scrollTo(0,((t-start)*0.6)%max);window.benchFrames++;requestAnimationFrame(frame)}requestAnimationFrame(frame);true";

/// WebGL 2 clearing the whole viewport and drawing a turning triangle.
/// `benchInfo()` also clears a WebGL 1 context and reads a pixel back.
const BENCH_WEBGL: &str = "document.body.innerHTML='';document.body.style.margin='0';\
let c=document.createElement('canvas');c.style='position:fixed;inset:0;width:100vw;height:100vh';document.body.append(c);\
let gl=c.getContext('webgl2');window.benchFrames=0;window.benchError=gl?null:'no WebGL 2 context';\
if(gl){let shader=(type,source)=>{let s=gl.createShader(type);gl.shaderSource(s,source);gl.compileShader(s);return s};\
let p=gl.createProgram();\
gl.attachShader(p,shader(gl.VERTEX_SHADER,'#version 300 es\\nuniform float t;in vec2 v;void main(){float c=cos(t),s=sin(t);gl_Position=vec4(mat2(c,-s,s,c)*v,0,1);}'));\
gl.attachShader(p,shader(gl.FRAGMENT_SHADER,'#version 300 es\\nprecision mediump float;uniform float t;out vec4 o;void main(){o=vec4(0.5+0.5*sin(t),0.64,0.42,1);}'));\
gl.linkProgram(p);gl.useProgram(p);gl.bindBuffer(gl.ARRAY_BUFFER,gl.createBuffer());\
gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([0,0.8,-0.7,-0.6,0.7,-0.6]),gl.STATIC_DRAW);\
let v=gl.getAttribLocation(p,'v');gl.enableVertexAttribArray(v);gl.vertexAttribPointer(v,2,gl.FLOAT,false,0,0);\
let u=gl.getUniformLocation(p,'t');\
function frame(t){let w=innerWidth*devicePixelRatio,h=innerHeight*devicePixelRatio;if(c.width!==w||c.height!==h){c.width=w;c.height=h}\
gl.viewport(0,0,w,h);gl.clearColor(0.06,0.13,0.1,1);gl.clear(gl.COLOR_BUFFER_BIT);gl.uniform1f(u,t/500);gl.drawArrays(gl.TRIANGLES,0,3);\
window.benchFrames++;requestAnimationFrame(frame)}requestAnimationFrame(frame)}\
window.benchInfo=()=>{let one=document.createElement('canvas').getContext('webgl'),pixel=null;\
if(one){one.clearColor(0.25,0.5,0.75,1);one.clear(one.COLOR_BUFFER_BIT);let b=new Uint8Array(4);\
one.readPixels(0,0,1,1,one.RGBA,one.UNSIGNED_BYTE,b);pixel=Array.from(b)}\
return {webgl2:!!gl,webgl1Pixel:pixel}};true";

/// A 60 fps video filling the viewport, played from a blob of the fixture's
/// `/bench.mp4`; its frames counted as the page composites them.
/// `benchInfo()` reports the decoder's frame counts.
const BENCH_VIDEO: &str = "document.body.innerHTML='';document.body.style.margin='0';document.body.style.background='#000';\
let v=document.createElement('video');v.muted=true;v.loop=true;v.playsInline=true;\
v.style='position:fixed;inset:0;width:100vw;height:100vh;object-fit:contain';window.benchFrames=0;window.benchError=null;\
v.onerror=()=>{window.benchError='video error '+(v.error&&v.error.code)};\
if(v.requestVideoFrameCallback){let f=()=>{window.benchFrames++;v.requestVideoFrameCallback(f)};v.requestVideoFrameCallback(f)}\
else{window.benchError='no requestVideoFrameCallback'}\
window.benchInfo=()=>{let q=v.getVideoPlaybackQuality();\
return {total:q.totalVideoFrames,dropped:q.droppedVideoFrames,width:v.videoWidth,height:v.videoHeight,paused:v.paused,\
ready:v.readyState,network:v.networkState,time:v.currentTime,mp4:v.canPlayType('video/mp4; codecs=\"avc1.640028\"')}};\
document.body.append(v);fetch('/bench.mp4').then(r=>r.blob())\
.then(b=>{v.src=URL.createObjectURL(b);return v.play()}).catch(e=>{window.benchError=String(e)});true";

/// A still page whose 80 px square changes colour on each press.
const BENCH_STILL: &str = "document.body.innerHTML='';document.body.style.margin='0';document.body.style.background='#10201a';\
let p=document.createElement('div');p.style='position:fixed;left:20px;top:20px;width:80px;height:80px;background:#3fa36b';\
let n=0;p.addEventListener('pointerdown',()=>{p.style.background=++n%2?'#f5f0df':'#3fa36b'});document.body.append(p);true";

/// The fixture and its descendants, the browser engine's processes.
fn processes() -> (u32, Vec<u32>) {
    let me = std::process::id();
    let mut parents = std::collections::HashMap::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // The parent is the second field after the parenthesized command name.
        let ppid = stat[stat.rfind(')').unwrap_or(0) + 2..]
            .split(' ')
            .nth(1)
            .and_then(|v| v.parse::<u32>().ok());
        parents.insert(pid, ppid.unwrap_or(0));
    }
    let descends = |mut pid: u32| {
        while let Some(&parent) = parents.get(&pid) {
            if parent == me {
                return true;
            }
            if parent <= 1 {
                return false;
            }
            pid = parent;
        }
        false
    };
    let engine = parents
        .keys()
        .copied()
        .filter(|pid| descends(*pid))
        .collect();
    (me, engine)
}

/// CPU seconds these processes have used so far.
fn cpu_seconds(pids: &[u32]) -> f64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    pids.iter()
        .filter_map(|pid| std::fs::read_to_string(format!("/proc/{pid}/stat")).ok())
        .map(|stat| {
            // utime and stime are the 14th and 15th fields overall.
            let fields: Vec<&str> = stat[stat.rfind(')').unwrap_or(0) + 2..]
                .split(' ')
                .collect();
            let field = |i: usize| {
                fields
                    .get(i - 3)
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.)
            };
            (field(14) + field(15)) / ticks
        })
        .sum()
}

/// Their proportional set size, in MiB: shared pages count once overall.
fn pss_mib(pids: &[u32]) -> f64 {
    pids.iter()
        .filter_map(|pid| std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok())
        .filter_map(|rollup| {
            let line = rollup.lines().find(|line| line.starts_with("Pss:"))?;
            line.split_whitespace().nth(1)?.parse::<f64>().ok()
        })
        .sum::<f64>()
        / 1024.
}

/// GPU memory NVIDIA's driver reports for each set of processes, in MiB.
fn gpu_mib(sets: &[&[u32]]) -> Option<Vec<f64>> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["-q", "-d", "PIDS"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut used = vec![0.; sets.len()];
    let mut pid = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "Process ID" => pid = value.trim().parse().unwrap_or(0),
            "Used GPU Memory" => {
                let mib = value
                    .trim()
                    .trim_end_matches("MiB")
                    .trim()
                    .parse()
                    .unwrap_or(0.);
                if let Some(set) = sets.iter().position(|set| set.contains(&pid)) {
                    used[set] += mib;
                }
            }
            _ => {}
        }
    }
    Some(used)
}

/// Counts the window's display frames until the returned counter is dropped.
fn count_display_frames(window: &gpui::Window) -> std::rc::Rc<std::cell::Cell<u32>> {
    fn next(window: &gpui::Window, count: std::rc::Rc<std::cell::Cell<u32>>) {
        window.on_next_display_frame(move |window, _| {
            if std::rc::Rc::strong_count(&count) > 1 {
                count.set(count.get() + 1);
                next(window, count);
            }
        });
    }
    let count = std::rc::Rc::default();
    next(window, std::rc::Rc::clone(&count));
    count
}

/// Frames per second over `millis`: the page's, counted by the script
/// `frames`, and the display's, from the window's frame callbacks.
async fn frame_rates(
    window: WindowHandle<shell::Shell>,
    page: &Entity<BrowserSurface>,
    frames: &str,
    millis: u64,
    cx: &mut AsyncApp,
) -> anyhow::Result<(f64, f64)> {
    let display = AnyWindowHandle::from(window).update(cx, |_, w, _| count_display_frames(w))?;
    let before = eval(page, frames, cx).await?.as_f64().unwrap_or(0.);
    let (start, shown) = (std::time::Instant::now(), display.get());
    pause(cx, millis).await;
    let after = eval(page, frames, cx).await?.as_f64().unwrap_or(0.);
    let elapsed = start.elapsed().as_secs_f64();
    Ok((
        (after - before) / elapsed,
        f64::from(display.get() - shown) / elapsed,
    ))
}

/// Milliseconds between `times`, two decimals.
fn intervals(times: &[std::time::Instant]) -> Vec<f64> {
    times
        .windows(2)
        .map(|pair| ((pair[1] - pair[0]).as_secs_f64() * 100_000.).round() / 100.)
        .collect()
}

/// The frames the page's script counted, when it counts them.
async fn page_frames(
    page: &Entity<BrowserSurface>,
    counted: bool,
    cx: &mut AsyncApp,
) -> anyhow::Result<Option<f64>> {
    if !counted {
        return Ok(None);
    }
    Ok(eval(page, "window.benchFrames", cx).await?.as_f64())
}

/// Loads a new document from the fixture's site, so nothing of the last
/// bench page keeps running.
async fn fresh_page(
    window: WindowHandle<shell::Shell>,
    page: &Entity<BrowserSurface>,
    url: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    eval(page, "document.title='stale';true", cx).await?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while page.read_with(cx, |b, _| b.page.title != "stale") {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the bench page kept its title"
        );
        pause(cx, 20).await;
    }
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| b.navigate(url, w, cx))
    })?;
    while !page.read_with(cx, |b, _| b.page.title == "Fieldnotes" && !b.page.loading) {
        anyhow::ensure!(std::time::Instant::now() < deadline, "{url} did not load");
        pause(cx, 20).await;
    }
    Ok(())
}

/// Runs `script` and measures `seconds` of the page after `warmup`: its frame
/// rate when the script counts frames, when GPUI painted each new frame, the
/// bytes copied to show them, and CPU, memory and GPU memory, apart for Zeron
/// and the engine.
async fn scenario(
    page: &Entity<BrowserSurface>,
    script: &str,
    counted: bool,
    warmup: u64,
    seconds: f64,
    cx: &mut AsyncApp,
) -> anyhow::Result<serde_json::Value> {
    eval(page, script, cx).await?;
    pause(cx, warmup).await;
    let (me, engine) = processes();
    let page_before = page_frames(page, counted, cx).await?;
    let painted_before = page.update(cx, |b, _| {
        b.fixture_linux_record(true);
        b.fixture_linux_painted()
    });
    let (zeron_before, engine_before) = (cpu_seconds(&[me]), cpu_seconds(&engine));
    let start = std::time::Instant::now();
    pause(cx, (seconds * 1000.) as u64).await;
    let (zeron_after, engine_after) = (cpu_seconds(&[me]), cpu_seconds(&engine));
    let elapsed = start.elapsed().as_secs_f64();
    let (painted, times) = page.update(cx, |b, _| {
        (b.fixture_linux_painted(), b.fixture_linux_record(false))
    });
    let page_after = page_frames(page, counted, cx).await?;
    let error = eval(page, "window.benchError||null", cx).await?;
    let info = eval(page, "typeof benchInfo=='function'?benchInfo():null", cx).await?;
    let shown = (painted.0 - painted_before.0) as f64;
    let per_frame = |bytes: u64| if shown > 0. { bytes as f64 / shown } else { 0. };
    let gpu = gpu_mib(&[&[me], &engine]);
    Ok(serde_json::json!({
        "seconds": elapsed,
        "error": error,
        "info": info,
        "pageFps": page_before.zip(page_after).map(|(before, after)| (after - before) / elapsed),
        "shownFps": shown / elapsed,
        "uploadedPerFrame": per_frame(painted.1 - painted_before.1),
        "copiedPerFrame": per_frame(painted.2 - painted_before.2),
        "zeronCpu": (zeron_after - zeron_before) / elapsed * 100.,
        "engineCpu": (engine_after - engine_before) / elapsed * 100.,
        "zeronPssMiB": pss_mib(&[me]),
        "enginePssMiB": pss_mib(&engine),
        "zeronGpuMiB": gpu.as_ref().map(|gpu| gpu[0]),
        "engineGpuMiB": gpu.as_ref().map(|gpu| gpu[1]),
        "engineProcesses": engine.len(),
        "intervalsMs": intervals(&times),
    }))
}

/// Presses the still page's square `samples` times, and returns how long
/// each press took to reach a frame GPUI painted.
async fn press_latency(
    window: WindowHandle<shell::Shell>,
    page: &Entity<BrowserSurface>,
    samples: usize,
    cx: &mut AsyncApp,
) -> anyhow::Result<serde_json::Value> {
    let square =
        page.read_with(cx, |b, _| b.fixture_linux_bounds().origin) + point(px(60.), px(60.));
    // The first press focuses the page.
    click(window, square, cx)?;
    pause(cx, 500).await;
    page.update(cx, |b, _| b.fixture_linux_record(true));
    let mut presses = Vec::new();
    for i in 0..samples {
        // Spread presses over the refresh, so none lines up with a vblank.
        pause(cx, 70 + (i as u64 * 37) % 61).await;
        let before = page.update(cx, |b, _| b.fixture_linux_painted()).0;
        presses.push(std::time::Instant::now());
        click(window, square, cx)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while page.update(cx, |b, _| b.fixture_linux_painted()).0 == before
            && std::time::Instant::now() < deadline
        {
            pause(cx, 2).await;
        }
    }
    pause(cx, 100).await;
    let times = page.update(cx, |b, _| b.fixture_linux_record(false));
    let latencies: Vec<Option<f64>> = presses
        .iter()
        .map(|press| {
            let painted = times.iter().find(|time| *time > press)?;
            Some(((*painted - *press).as_secs_f64() * 100_000.).round() / 100.)
        })
        .collect();
    Ok(serde_json::json!({ "samplesMs": latencies }))
}

/// `ZERON_BROWSER_BENCH=<seconds>`: measures this run's engine and frame path
/// on pages that repaint all of a large viewport every frame (a 2D canvas,
/// WebGL 2, scrolling) and only a small part of it (a CSS spinner), a 60 fps
/// video when `ZERON_BROWSER_BENCH_VIDEO` names an MP4, how long a press takes
/// to show, and what an idle page costs. `ZERON_BROWSER_BENCH_WIDTH` sets the
/// browser's width in logical pixels. Writes `bench.json` and a capture of
/// each page.
pub async fn bench(
    window: WindowHandle<shell::Shell>,
    page: Entity<BrowserSurface>,
    origin: &str,
    output: &std::path::Path,
    seconds: f64,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let width = std::env::var("ZERON_BROWSER_BENCH_WIDTH")
        .ok()
        .and_then(|w| w.parse().ok())
        .unwrap_or(1400.);
    window.update(cx, |s, _, cx| s.fixture_resize_browser(width, cx))?;
    pause(cx, 1000).await;
    let mut scenarios = serde_json::Map::new();
    let video = std::env::var_os("ZERON_BROWSER_BENCH_VIDEO").is_some();
    for (name, script, counted) in [
        ("canvas", BENCH_CANVAS, true),
        ("webgl", BENCH_WEBGL, true),
        ("scroll", BENCH_SCROLL, true),
        ("css", BENCH_CSS, false),
        ("video", BENCH_VIDEO, true),
    ] {
        if name == "video" && !video {
            continue;
        }
        fresh_page(window, &page, &format!("{origin}/bench-{name}"), cx).await?;
        scenarios.insert(
            name.into(),
            scenario(&page, script, counted, 2000, seconds, cx).await?,
        );
        super::capture_frame(window, output, &format!("bench-{name}"), cx).await?;
    }
    fresh_page(window, &page, &format!("{origin}/bench-still"), cx).await?;
    eval(&page, BENCH_STILL, cx).await?;
    pause(cx, 1000).await;
    let latency = press_latency(window, &page, 40, cx).await?;
    // Idle: the still page, once WebKit has stopped asking for vblanks.
    pause(cx, 1000).await;
    let idle = scenario(&page, "true", false, 0, seconds.min(5.), cx).await?;
    let scale = AnyWindowHandle::from(window).update(cx, |_, w, _| w.scale_factor())?;
    let bounds = page.read_with(cx, |b, _| b.fixture_linux_bounds());
    let result = serde_json::json!({
        "engine": page.read_with(cx, |b, _| if b.fixture_linux_frames().is_null() { "gtk" } else { "wpe" }),
        "frames": page.read_with(cx, |b, _| b.fixture_linux_frames()),
        "viewport": [(f32::from(bounds.size.width) * scale).round(), (f32::from(bounds.size.height) * scale).round()],
        "scale": scale,
        "scenarios": scenarios,
        "latency": latency,
        "idle": idle,
    });
    std::fs::write(
        output.join("bench.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(())
}

pub async fn exercise(
    window: WindowHandle<shell::Shell>,
    page: Entity<BrowserSurface>,
    output: &std::path::Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    eval(&page,"document.body.style.paddingTop='0'; document.body.insertAdjacentHTML('afterbegin', `<div id='browser-blur-grid' style='height:140px;background:repeating-conic-gradient(#172f25 0% 25%,#f5f0df 0% 50%) 0 0/16px 16px'></div><input id='browser-input' style='height:32px;width:200px' placeholder='Browser input'><button id='browser-button' onclick='window.browserClicks=(window.browserClicks||0)+1'>Click</button>`); window.browserClicks=0; document.body.addEventListener('pointerdown',()=>window.pagePresses=(window.pagePresses||0)+1); let live=document.createElement('div');live.style='position:fixed;bottom:12px;right:12px;background:#29483b;color:white;padding:8px;border-radius:6px;font:12px monospace';document.body.append(live);window.browserFrames=0;function frame(){live.textContent='LIVE '+(++window.browserFrames);requestAnimationFrame(frame)}frame();true",cx).await?;
    pause(cx, 400).await;
    let bounds = page.read_with(cx, |b, _| b.fixture_linux_bounds());
    let input=eval(&page,"(()=>{let r=document.getElementById('browser-input').getBoundingClientRect();return [r.x+20,r.y+15]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(input[0].as_f64().unwrap() as f32),
                px(input[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    // The native click reaches WebKit through the X server after xdotool
    // returns. Wait for the input to take focus instead of racing it with a
    // fixed pause: a loaded runner lost the first keystroke to that race.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while eval(&page, "document.activeElement?.id === 'browser-input'", cx).await? != true {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "click did not focus the browser input"
        );
        pause(cx, 20).await;
    }
    for c in "hello linux".chars() {
        let key = if c == ' ' {
            "space".into()
        } else {
            c.to_string()
        };
        let stroke = gpui::Keystroke {
            key,
            key_char: Some(c.to_string()),
            modifiers: Default::default(),
        };
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: stroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        dispatch(
            window,
            PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke: stroke }),
            cx,
        )?;
        pause(cx, 35).await;
    }
    let typed = eval(&page, "document.getElementById('browser-input').value", cx).await?;
    anyhow::ensure!(
        typed == "hello linux",
        "GPUI keyboard input did not reach WebKit: {typed}"
    );
    if let Ok(id) = std::env::var("ZERON_BROWSER_CAPTURE_WINDOW") {
        let scale = AnyWindowHandle::from(window).update(cx, |_, w, _| w.scale_factor())?;
        let x = ((f32::from(bounds.origin.x) + input[0].as_f64().unwrap() as f32) * scale) as i32;
        let y = ((f32::from(bounds.origin.y) + input[1].as_f64().unwrap() as f32) * scale) as i32;
        std::process::Command::new("xdotool")
            .args([
                "mousemove",
                "--window",
                &id,
                &x.to_string(),
                &y.to_string(),
                "click",
                "1",
            ])
            .status()?;
        pause(cx, 200).await;
        // Restore caret to end after the real seat-focus click.
        eval(&page,"(()=>{let e=document.getElementById('browser-input');e.setSelectionRange(e.value.length,e.value.length);return true})()",cx).await?;
    }
    // Compositors give an unfocused window no clipboard: local runs on a hidden
    // workspace (`ZERON_BROWSER_FIXTURE_UNFOCUSED`) type that text instead.
    let clipboard = std::env::var_os("ZERON_BROWSER_FIXTURE_UNFOCUSED").is_none();
    if clipboard {
        cx.update(|cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(" — café 日本語".into()))
        });
        pause(cx, 200).await;
        let paste = gpui::Keystroke::parse("ctrl-v")?;
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: paste,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        pause(cx, 150).await;
        anyhow::ensure!(
            eval(&page, "document.getElementById('browser-input').value", cx).await?
                == "hello linux — café 日本語",
            "Unicode clipboard paste failed: {:?}",
            eval(&page, "document.getElementById('browser-input').value", cx).await?
        );
        eval(
            &page,
            "document.getElementById('browser-input').select();true",
            cx,
        )
        .await?;
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("ctrl-c")?,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        pause(cx, 150).await;
        anyhow::ensure!(
            cx.update(|cx| cx.read_from_clipboard().and_then(|v| v.text()))
                .as_deref()
                == Some("hello linux — café 日本語"),
            "browser copy did not reach app clipboard"
        );
    } else {
        eval(&page, "(()=>{let e=document.getElementById('browser-input');e.value+=' — café 日本語';return true})()", cx).await?;
    }
    eval(&page,"(()=>{let e=document.getElementById('browser-input');e.setSelectionRange(e.value.length,e.value.length);return true})()",cx).await?;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(b, None, "にほ", None, w, cx)
        })
    })?;
    pause(cx, 100).await;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(b, None, "日本語", None, w, cx)
        })
    })?;
    pause(cx, 100).await;
    capture_frame(window, output, "linux-ime-dark", cx).await?;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_text_in_range(b, None, "日本語", w, cx)
        })
    })?;
    pause(cx, 150).await;
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-input').value", cx).await?
            == "hello linux — café 日本語日本語",
        "IME composition did not commit correctly"
    );
    eval(&page,"document.getElementById('browser-button').insertAdjacentHTML('afterend','<select id=browser-select><option value=a>Alpha</option><option value=b>Beta</option></select>');true",cx).await?;
    let select=eval(&page,"(()=>{let r=document.getElementById('browser-select').getBoundingClientRect();return [r.x+10,r.y+10]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(select[0].as_f64().unwrap() as f32),
                px(select[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "HTML select did not open a GPUI menu"
    );
    capture_frame(window, output, "linux-select-dark", cx).await?;
    for key in ["down", "enter"] {
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key)?,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        pause(cx, 100).await;
    }
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-select').value", cx).await? == "b",
        "select keyboard navigation failed"
    );
    let context = bounds.origin + point(px(300.), px(330.));
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: context,
            button: MouseButton::Right,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: context,
            button: MouseButton::Right,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "context menu did not open in GPUI"
    );
    capture_frame(window, output, "linux-context-dark", cx).await?;
    dispatch(
        window,
        PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("escape")?,
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    pause(cx, 150).await;
    anyhow::ensure!(
        !page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "context menu did not dismiss"
    );
    for _ in 0..5 {
        for pos in [
            point(bounds.origin.x + px(25.), px(20.)),
            point(bounds.origin.x + px(75.), px(56.)),
            point(bounds.origin.x + px(180.), px(20.)),
        ] {
            dispatch(
                window,
                PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: pos,
                    ..Default::default()
                }),
                cx,
            )?;
            pause(cx, 120).await;
            anyhow::ensure!(
                page.read_with(cx, |b, _| b.fixture_native_visible()),
                "hover hid the browser"
            );
        }
    }
    dispatch(
        window,
        PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position: point(bounds.origin.x + px(75.), px(56.)),
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 800).await;
    capture_frame(window, output, "linux-tooltip-dark", cx).await?;
    let button=eval(&page,"(()=>{let r=document.getElementById('browser-button').getBoundingClientRect();return [r.x+15,r.y+10]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(button[0].as_f64().unwrap() as f32),
                px(button[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    pause(cx, 100).await;
    anyhow::ensure!(
        eval(&page, "window.browserClicks", cx).await? == 1,
        "page button did not activate"
    );
    dispatch(
        window,
        PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
            position: bounds.origin + point(px(300.), px(300.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-250.))),
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        eval(&page, "scrollY", cx).await?.as_f64().unwrap() > 50.,
        "scroll did not reach page: {}",
        eval(&page, "JSON.stringify({y: scrollY, height: document.scrollingElement.scrollHeight, viewport: innerHeight, frames: window.browserFrames})", cx).await?
    );
    eval(&page, "scrollTo(0,0);true", cx).await?;
    pause(cx, 200).await;
    let start = point(bounds.origin.x - px(2.), bounds.origin.y + px(100.));
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    let mut widths = Vec::new();
    for delta in [
        20., 60., 100., 140., 100., 60., 20., -20., -60., -100., -140., -100., -60., -20., 0.,
    ] {
        dispatch(
            window,
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: start + point(px(delta), px(0.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        )?;
        pause(cx, 100).await;
        anyhow::ensure!(
            cx.update(|cx| cx.has_active_drag()),
            "resize failed to start"
        );
        widths.push(f32::from(
            page.read_with(cx, |b, _| b.fixture_linux_bounds().size.width),
        ));
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "resizing hid browser"
        );
    }
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 400).await;
    anyhow::ensure!(
        widths.iter().copied().fold(f32::MIN, f32::max)
            - widths.iter().copied().fold(f32::MAX, f32::min)
            > 150.,
        "resize did not change viewport"
    );
    let width = eval(&page, "innerWidth", cx).await?.as_f64().unwrap();
    let layout = f32::from(page.read_with(cx, |b, _| b.fixture_linux_bounds().size.width));
    anyhow::ensure!(
        (width - layout as f64).abs() < 2.,
        "CSS viewport {width} differs from GPUI {layout}"
    );
    capture_frame(window, output, "linux-resize-dark", cx).await?;
    for _ in 0..4 {
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(false, cx))?;
        pause(cx, 400).await;
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "left sidebar lost browser"
        );
    }
    for _ in 0..3 {
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
        pause(cx, 60).await;
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
        pause(cx, 400).await;
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "interrupted sidebar lost browser"
        );
    }
    window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
    pause(cx, 400).await;
    anyhow::ensure!(
        !page.read_with(cx, |b, _| b.fixture_native_visible()),
        "closed sidebar kept browser active"
    );
    window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
    pause(cx, 400).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_native_visible()),
        "reopened sidebar lost browser"
    );
    // The animating page renders again as soon as it is shown, before anything
    // else wakes it.
    let frames = page.read_with(cx, |b, _| b.fixture_linux_frames());
    if !frames.is_null() {
        let since = frames["vblankMs"].as_f64();
        anyhow::ensure!(
            since.is_some_and(|ms| ms < 100.),
            "a page shown again gets no vblanks (last {since:?} ms ago)"
        );
    }
    cx.update(|cx| appearance::set_surface(zeron_theme::SurfacePreference::Frosted, cx));
    capture_frame(window, output, "browser-blur-baseline-dark", cx).await?;
    window.update(cx, |s, _, cx| s.fixture_browser_menu(true, cx))?;
    pause(cx, 700).await;
    let presses = eval(&page, "window.pagePresses||0", cx).await?;
    let outside =
        page.read_with(cx, |b, _| b.fixture_linux_bounds().origin) + point(px(350.), px(300.));
    click(window, outside, cx)?;
    // Dismissal includes an exit animation and a deferred repaint. Wait for
    // the real popup lifecycle instead of racing its 120ms removal timer.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while window.update(cx, |s, _, _| s.fixture_browser_menu_mounted())? {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "outside click did not dismiss browser menu"
        );
        pause(cx, 20).await;
    }
    pause(cx, 100).await;
    anyhow::ensure!(
        eval(&page, "window.pagePresses||0", cx).await? == presses,
        "outside menu click leaked into browser"
    );
    click(window, outside, cx)?;
    pause(cx, 150).await;
    anyhow::ensure!(
        eval(&page, "window.pagePresses||0", cx).await?.as_u64()
            == Some(presses.as_u64().unwrap() + 1),
        "browser input did not return after menu dismissal"
    );
    window.update(cx, |s, _, cx| s.fixture_browser_menu(true, cx))?;
    pause(cx, 500).await;
    capture_frame(window, output, "browser-blur-dark", cx).await?;
    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
    pause(cx, 500).await;
    capture_frame(window, output, "browser-blur-light", cx).await?;
    eval(
        &page,
        "document.getElementById('browser-blur-grid').style.background='#263d35'",
        cx,
    )
    .await?;
    pause(cx, 300).await;
    capture_frame(window, output, "browser-blur-solid-light", cx).await?;
    cx.update(|cx| appearance::set_surface(zeron_theme::SurfacePreference::Opaque, cx));
    pause(cx, 300).await;
    capture_frame(window, output, "browser-menu-opaque", cx).await?;
    let bounds = page.read_with(cx, |b, _| b.fixture_linux_bounds());
    let viewport =
        AnyWindowHandle::from(window).update(cx, |_, w, _| f32::from(w.viewport_size().width))?;
    super::validate_blur(
        output,
        // Browser and Terminal are the two permanent rows. Keep this in sync
        // with the compact menu so the lower-right sample stays inside it.
        (f32::from(bounds.origin.x) as f64 + 124., 42., 168., 78.),
        viewport,
    )?;
    window.update(cx, |s, _, cx| s.fixture_browser_menu(false, cx))?;
    cx.update(|cx| {
        appearance::set_mode(appearance::AppearanceMode::Dark, cx);
        appearance::set_surface(zeron_theme::SurfacePreference::Frosted, cx);
    });
    pause(cx, 300).await;
    let frames = eval(&page, "window.browserFrames", cx)
        .await?
        .as_u64()
        .unwrap();
    anyhow::ensure!(frames > 60, "page stopped rendering during interaction");
    // An animating page at rest renders once per display frame: WPE at the
    // display's rate, WebKitGTK, the fallback, at up to 60 Hz.
    let (frame_rate, display_rate) =
        frame_rates(window, &page, "window.browserFrames", 2000, cx).await?;
    let path = page.read_with(cx, |b, _| b.fixture_linux_frames());
    let expected = if path.is_null() {
        display_rate.min(60.)
    } else {
        display_rate
    };
    anyhow::ensure!(
        frame_rate > expected * 0.9,
        "the page renders {frame_rate:.1} frames/s on a display showing {display_rate:.1}"
    );
    // WebKit keeps about four buffers per page; more means its frames outpace
    // the display.
    if let Some(shared) = path["shared"].as_u64() {
        anyhow::ensure!(shared <= 6, "WebKit keeps {shared} buffers for one page");
    }
    std::fs::write(
        output.join("linux-results.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"keyboard":true,"unicodeClipboard":clipboard,"imeComposition":true,"buttonAndScroll":true,"menuClickIsolation":true,"hoverVisibility":true,"resizeWidths":widths,"cssViewport":width,"liveFrames":frames,"frameRate":frame_rate,"displayRate":display_rate,"frames":path,"sidebarTransitions":true}),
        )?,
    )?;
    Ok(())
}
