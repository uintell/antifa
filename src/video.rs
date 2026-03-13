use std::io::ErrorKind;
use std::process::Stdio;

use anyhow::{anyhow, Context};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

#[cfg(target_os = "linux")]
const DEFAULT_LINUX_DEVICE: &str = "/dev/video0";
#[cfg(target_os = "linux")]
const DEFAULT_FRAME_RATE: &str = "2";
#[cfg(target_os = "linux")]
const DEFAULT_FRAME_SIZE: &str = "320x240";
#[cfg(target_os = "linux")]
const DEFAULT_X11GRAB_SIZE: &str = "1024x576";

pub fn spawn_capture_process() -> anyhow::Result<Child> {
    let mut command = Command::new("ffmpeg");
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());
    command.args(["-hide_banner", "-loglevel", "error"]);
    apply_capture_input(&mut command)?;
    command.args([
        "-vf",
        "fps=2,scale=320:-1:flags=lanczos",
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "pipe:1",
    ]);

    command
        .spawn()
        .context("spawn ffmpeg capture process for local video")
}

#[cfg(target_os = "linux")]
fn apply_capture_input(command: &mut Command) -> anyhow::Result<()> {
    if let Ok(device) = std::env::var("ANTIFA_VIDEO_DEVICE") {
        return apply_linux_v4l2_capture(command, &device);
    }

    if std::path::Path::new(DEFAULT_LINUX_DEVICE).exists() {
        return apply_linux_v4l2_capture(command, DEFAULT_LINUX_DEVICE);
    }

    let display = std::env::var("DISPLAY")
        .map_err(|_| anyhow!("no /dev/video0 found and DISPLAY is not set for X11 fallback"))?;
    let display_input = if display.contains('.') || display.contains('+') {
        display
    } else {
        format!("{display}.0")
    };
    let size =
        std::env::var("ANTIFA_X11GRAB_SIZE").unwrap_or_else(|_| DEFAULT_X11GRAB_SIZE.into());
    command.args([
        "-f",
        "x11grab",
        "-framerate",
        DEFAULT_FRAME_RATE,
        "-video_size",
    ]);
    command.arg(size);
    command.args(["-draw_mouse", "1", "-i"]);
    command.arg(display_input);
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_linux_v4l2_capture(command: &mut Command, device: &str) -> anyhow::Result<()> {
    command.args([
        "-f",
        "v4l2",
        "-framerate",
        DEFAULT_FRAME_RATE,
        "-video_size",
        DEFAULT_FRAME_SIZE,
        "-i",
    ]);
    command.arg(device);
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_capture_input(command: &mut Command) -> anyhow::Result<()> {
    let source = std::env::var("ANTIFA_VIDEO_INPUT").map_err(|_| {
        anyhow!(
            "set ANTIFA_VIDEO_INPUT to a DirectShow input, for example video=Integrated Camera"
        )
    })?;

    command.args(["-f", "dshow", "-i"]);
    command.arg(source);
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn apply_capture_input(_command: &mut Command) -> anyhow::Result<()> {
    Err(anyhow!(
        "local video capture is only implemented for Linux and Windows"
    ))
}

pub async fn read_png_frame<R>(reader: &mut R) -> anyhow::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut signature = [0u8; 8];
    match reader.read_exact(&mut signature).await {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("read PNG signature from ffmpeg"),
    }

    let png_signature = [137, 80, 78, 71, 13, 10, 26, 10];
    if signature != png_signature {
        return Err(anyhow!("ffmpeg did not emit a PNG frame"));
    }

    let mut frame = signature.to_vec();

    loop {
        let mut length_bytes = [0u8; 4];
        reader
            .read_exact(&mut length_bytes)
            .await
            .context("read PNG chunk length")?;
        frame.extend_from_slice(&length_bytes);

        let chunk_len = u32::from_be_bytes(length_bytes) as usize;
        let mut chunk_type = [0u8; 4];
        reader
            .read_exact(&mut chunk_type)
            .await
            .context("read PNG chunk type")?;
        frame.extend_from_slice(&chunk_type);

        let mut chunk_data = vec![0u8; chunk_len + 4];
        reader
            .read_exact(&mut chunk_data)
            .await
            .context("read PNG chunk data")?;
        frame.extend_from_slice(&chunk_data);

        if chunk_type == *b"IEND" {
            return Ok(Some(frame));
        }
    }
}
