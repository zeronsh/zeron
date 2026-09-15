use crate::*;
use ironrdp::{
    connector::{self, Credentials},
    pdu::{
        gcc::KeyboardType,
        rdp::{
            capability_sets::MajorPlatformType,
            client_info::{PerformanceFlags, TimezoneInfo},
        },
    },
    session::{ActiveStageBuilder, ActiveStageOutput, image::DecodedImage},
};
use ironrdp_tokio::{FramedWrite, NetworkClient, TokioFramed};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{
    net::{TcpStream, lookup_host},
    time::{Duration, timeout},
};

/// Starts exactly one local worker. A dedicated Tokio executor isolates decoder
/// work from both GPUI and the application's shared async workers.
pub fn connect(config: ConnectConfig, generation: u64) -> Result<SessionHandle, SessionError> {
    validate_size(config.width, config.height)?;
    let (handle, mut channels) = SessionHandle::channel(generation);
    channels
        .snapshots
        .send_modify(|s| s.state = SessionState::Connecting);
    std::thread::Builder::new()
        .name(format!("rdp-{generation}"))
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    channels.snapshots.send_modify(|s| {
                        s.state = SessionState::Failed(SessionError::new(
                            ErrorStage::Session,
                            "Could not start RDP worker",
                        ))
                    });
                    return;
                }
            };
            runtime.block_on(async {
                channels
                    .snapshots
                    .send_modify(|s| s.state = SessionState::Connecting);
                let result = run(config, &mut channels).await;
                channels.snapshots.send_modify(|s| {
                    s.frame = None;
                    s.cursor = RemoteCursor::Default;
                    s.certificate = None;
                    s.state = match result {
                        Ok(()) => SessionState::Disconnected,
                        Err(e) => SessionState::Failed(e),
                    };
                });
            });
        })
        .map_err(|_| SessionError::new(ErrorStage::Session, "Could not start RDP worker"))?;
    Ok(handle)
}

// Default CredSSP uses NTLM. Never allow an RDP endpoint to redirect credentials
// to an arbitrary Kerberos/HTTP destination through a NetworkRequest.
struct DirectOnly;
impl NetworkClient for DirectOnly {
    async fn send(
        &mut self,
        _: &connector::sspi::generator::NetworkRequest,
    ) -> connector::ConnectorResult<Vec<u8>> {
        Err(connector::general_err!(
            "External authentication service is unsupported for direct RDP"
        ))
    }
}
fn error(stage: ErrorStage, detail: impl std::fmt::Display) -> SessionError {
    SessionError::new(stage, detail.to_string())
}
async fn limited<T>(
    duration: Duration,
    stage: ErrorStage,
    future: impl std::future::Future<Output = Result<T, impl std::fmt::Display>>,
) -> Result<T, SessionError> {
    timeout(duration, future)
        .await
        .map_err(|_| error(stage, "Timed out"))?
        .map_err(|e| error(stage, e))
}

pub(crate) fn connector_config(config: &ConnectConfig) -> connector::Config {
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username: config.username.clone(),
            password: config.password.expose().to_string(),
        },
        domain: config.domain.clone(),
        enable_tls: true,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: config.keyboard_layout,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: config.width,
            height: config.height,
        },
        bitmap: None,
        // xrdp treats build <=419 as a legacy client and silently skips the
        // reactivation sequence, even after advertising Display Control.
        client_build: 6000,
        client_name: "Zeron".into(),
        client_dir: String::new(),
        platform: if cfg!(target_os = "macos") {
            MajorPlatformType::MACINTOSH
        } else {
            MajorPlatformType::UNIX
        },
        enable_server_pointer: true,
        pointer_software_rendering: false,
        request_data: None,
        autologon: true,
        enable_audio_playback: false,
        compression_type: None,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}
type Transport = TokioFramed<tokio_rustls::client::TlsStream<TcpStream>>;
struct Established {
    framed: Transport,
    result: connector::ConnectionResult,
    clipboard: crate::clipboard::SharedClipboard,
    server_area: Arc<AtomicU64>,
}
async fn establish(
    config: &ConnectConfig,
    channels: &mut SessionChannels,
) -> Result<Established, SessionError> {
    let addresses: Vec<_> = limited(
        config.timeout,
        ErrorStage::Dns,
        lookup_host((config.host.as_str(), config.port)),
    )
    .await?
    .collect();
    if addresses.is_empty() {
        return Err(error(ErrorStage::Dns, "No addresses found"));
    }
    let stream = limited(
        config.timeout,
        ErrorStage::Network,
        TcpStream::connect(addresses.as_slice()),
    )
    .await?;
    stream
        .set_nodelay(true)
        .map_err(|e| error(ErrorStage::Network, e))?;
    let local = stream
        .local_addr()
        .map_err(|e| error(ErrorStage::Network, e))?;
    let mut connector = connector::ClientConnector::new(connector_config(&config), local);
    let clipboard = crate::clipboard::SharedClipboard::default();
    connector.attach_static_channel(ironrdp::cliprdr::CliprdrClient::new(Box::new(
        crate::clipboard::Backend(clipboard.clone()),
    )));
    let server_area = Arc::new(AtomicU64::new(0));
    connector.attach_static_channel(
        ironrdp::dvc::DrdynvcClient::new()
            .with_dynamic_channel(crate::display_control::DisplayControl(server_area.clone())),
    );
    let mut framed = TokioFramed::new(stream);
    let upgrade = limited(
        config.timeout,
        ErrorStage::Protocol,
        ironrdp_tokio::connect_begin(&mut framed, &mut connector),
    )
    .await?;
    let (stream, public_key, challenge) = limited(
        config.timeout,
        ErrorStage::Certificate,
        crate::tls::upgrade(
            framed.into_inner_no_leftover(),
            &config.host,
            config.port,
            config.trusted_certificate_sha256.clone(),
        ),
    )
    .await?;
    if let Some(challenge) = challenge {
        channels.snapshots.send_modify(|s| {
            s.state = SessionState::AwaitingCertificateDecision;
            s.certificate = Some(challenge.clone());
        });
        loop {
            match channels.commands.recv().await {
                Some(Command::Certificate(CertificateDecision::TrustOnce)) => break,
                Some(Command::Certificate(CertificateDecision::SavePin)) => {
                    channels
                        .snapshots
                        .send_modify(|s| s.accepted_pin = Some(challenge.sha256.clone()));
                    break;
                }
                Some(Command::Certificate(CertificateDecision::Reject)) | None => {
                    return Err(error(ErrorStage::Certificate, "Certificate rejected"));
                }
                _ => {}
            }
        }
    }
    channels.snapshots.send_modify(|s| {
        s.state = SessionState::Authenticating;
        s.certificate = None;
    });
    let upgraded = ironrdp_tokio::mark_as_upgraded(upgrade, &mut connector);
    let mut framed = TokioFramed::new(stream);
    let result = limited(
        config.timeout,
        ErrorStage::Authentication,
        ironrdp_tokio::connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut DirectOnly,
            config.host.clone().into(),
            public_key,
            None,
        ),
    )
    .await?;
    Ok(Established {
        framed,
        result,
        clipboard,
        server_area,
    })
}
async fn run(config: ConnectConfig, channels: &mut SessionChannels) -> Result<(), SessionError> {
    let cancellation = channels.cancellation.clone();
    let Established {
        mut framed,
        result,
        clipboard,
        server_area,
    } = tokio::select! {
        biased;
        _=cancellation.cancelled()=>return Ok(()),
        result=establish(&config,channels)=>result?,
    };
    validate_size(result.desktop_size.width, result.desktop_size.height)?;
    let mut image = DecodedImage::new(
        ironrdp::graphics::image_processing::PixelFormat::BgrA32,
        result.desktop_size.width,
        result.desktop_size.height,
    );
    let activation_factory = result.activation_factory;
    let mut active = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: false,
    }
    .build();
    channels
        .snapshots
        .send_modify(|s| s.state = SessionState::Connected);
    let mut publisher = crate::graphics::Publisher::new(channels.snapshots.borrow().generation);
    let mut tick = tokio::time::interval(Duration::from_millis(34));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut input = crate::input::InputState::new(config.keyboard_layout);
    let mut resize = crate::resize::ResizeDebounce::default();
    let mut last_pointer = None;
    let mut visible = true;
    let mut received_graphics = false;
    let started = Instant::now();
    let mut graphics_started = Instant::now();
    let mut last_diagnostic = Instant::now();
    let mut pdus = 0u64;
    let mut bytes_received = 0u64;
    let mut publications = 0u64;
    loop {
        pump_clipboard(&clipboard, &mut active, &mut framed, channels).await?;
        tokio::select! {
            _=cancellation.cancelled()=>{
                channels.snapshots.send_modify(|s|s.state=SessionState::Closing);
                let _=timeout(Duration::from_millis(250),async {
                    send_input(&mut active,&mut image,&mut framed,&input.release()).await?;
                    use tokio::io::AsyncWriteExt;
                    framed.get_inner_mut().0.shutdown().await.map_err(|e|error(ErrorStage::Network,e))
                }).await;
                return Ok(());
            }
            command = channels.commands.recv() => {
                let events = match command {
                    Some(Command::Input(event)) => input.apply(event),
                    Some(Command::ReleaseAll) => input.release(),
                    Some(Command::CtrlAltDelete) => input.ctrl_alt_delete(),
                    Some(Command::SendClipboard(text)) => {
                        let offered=clipboard.lock().unwrap().offer(text);
                        match offered {
                            Ok(()) => {
                                if let Some(channel)=active.get_svc_processor_mut::<ironrdp::cliprdr::CliprdrClient>() {
                                    let messages=channel.initiate_copy(&[ironrdp::cliprdr::pdu::ClipboardFormat::new(ironrdp::cliprdr::pdu::ClipboardFormatId::CF_UNICODETEXT)]).map_err(|e|error(ErrorStage::Clipboard,e))?;
                                    let bytes=active.process_svc_processor_messages(messages).map_err(|e|error(ErrorStage::Clipboard,e))?;
                                    limited(Duration::from_secs(2),ErrorStage::Network,framed.write_all(&bytes)).await?;
                                }
                            }
                            Err(e)=>channels.snapshots.send_modify(|s|s.clipboard=Some((0,Err(e)))),
                        }
                        Vec::new()
                    }
                    Some(Command::RequestClipboard(id)) => {
                        let requested=clipboard.lock().unwrap().request(id);
                        match requested {
                            Ok(()) => {
                                if let Some(channel)=active.get_svc_processor_mut::<ironrdp::cliprdr::CliprdrClient>() {
                                    let messages=channel.initiate_paste(ironrdp::cliprdr::pdu::ClipboardFormatId::CF_UNICODETEXT).map_err(|e|error(ErrorStage::Clipboard,e))?;
                                    let bytes=active.process_svc_processor_messages(messages).map_err(|e|error(ErrorStage::Clipboard,e))?;
                                    limited(Duration::from_secs(2),ErrorStage::Network,framed.write_all(&bytes)).await?;
                                }
                            }
                            Err(e)=>channels.snapshots.send_modify(|s|s.clipboard=Some((id,Err(e)))),
                        }
                        Vec::new()
                    }
                    None => return Ok(()),
                    _ => Vec::new(),
                };
                send_input(&mut active, &mut image, &mut framed, &events).await?;
            }
            _ = tick.tick() => {
                if !received_graphics && graphics_started.elapsed()>config.timeout {return Err(error(ErrorStage::Protocol,"Server connected but did not provide a desktop image"));}
                if last_diagnostic.elapsed()>Duration::from_secs(10) {
                    if std::env::var_os("ZERON_RDP_DIAGNOSTICS").is_some() {tracing::info!(generation=channels.snapshots.borrow().generation,pdus,bytes_received,publications,visible,elapsed_ms=started.elapsed().as_millis() as u64,"Local RDP session statistics");}
                    last_diagnostic=Instant::now();
                }
                let area=if received_graphics {server_area.load(Ordering::Relaxed)} else {0};
                if area>0 && !channels.snapshots.borrow().capabilities.resize {channels.snapshots.send_modify(|s|s.capabilities.resize=true);}
                if let Some((width,height))=resize.take_ready(Instant::now(),area,(image.width(),image.height())) {
                    let encoded=crate::display_control::encode_resize(&mut active,width,height)?;
                    tracing::debug!(width,height,server_area=area,available=encoded.is_some(),"Remote desktop resize requested");
                    if let Some(bytes)=encoded {
                        limited(Duration::from_secs(2),ErrorStage::Network,framed.write_all(&bytes)).await?;
                    }
                }
                if let Some(frame) = publisher.publish(&image, channels.presentation.borrow().visible, Instant::now())? {
                    publications+=1;
                    channels.snapshots.send_modify(|s| {s.frame = Some(frame);s.reactivating=false;});
                }
            }
            changed = channels.presentation.changed() => {
                if changed.is_err() { return Ok(()); }
                let presentation = channels.presentation.borrow_and_update().clone();
                resize.update(presentation.resize,presentation.visible,Instant::now());
                if visible != presentation.visible {
                    visible = presentation.visible;
                    if visible { publisher.mark_dirty(); }
                    else { send_input(&mut active,&mut image,&mut framed,&input.release()).await?; channels.snapshots.send_modify(|s| s.frame = None); }
                }
                if visible && presentation.pointer != last_pointer {
                    last_pointer = presentation.pointer;
                    if let Some((x,y)) = last_pointer { send_input(&mut active,&mut image,&mut framed,&input.pointer(x,y)).await?; }
                }
            }
            packet = framed.read_pdu() => {
                let (action, packet) = packet.map_err(|e| error(ErrorStage::Session, e))?;
                pdus+=1;bytes_received+=packet.len() as u64;
                let outputs = if crate::resize::short_deactivation(action,&packet,activation_factory.io_channel_id()) {
                    vec![ActiveStageOutput::DeactivateAll]
                } else {active.process(&mut image, action, &packet).map_err(|e| error(ErrorStage::Protocol,e))?};
                for output in outputs {
                    match output {
                        ActiveStageOutput::ResponseFrame(bytes) => { if !bytes.is_empty() { limited(Duration::from_secs(5), ErrorStage::Network, framed.write_all(&bytes)).await?; } }
                        ActiveStageOutput::GraphicsUpdate(_) => {received_graphics=true;publisher.mark_dirty();},
                        ActiveStageOutput::PointerDefault => {channels.snapshots.send_if_modified(|s| {s.cursor = RemoteCursor::Default;visible});},
                        ActiveStageOutput::PointerHidden => {channels.snapshots.send_if_modified(|s| {s.cursor = RemoteCursor::Hidden;visible});},
                        ActiveStageOutput::PointerBitmap(pointer) => {
                            let cursor = RemoteCursor::Bitmap { width: pointer.width, height: pointer.height, hotspot_x: pointer.hotspot_x, hotspot_y: pointer.hotspot_y, rgba: Arc::from(pointer.bitmap_data.as_slice()) };
                            channels.snapshots.send_if_modified(|s| {s.cursor = cursor;visible});
                        }
                        ActiveStageOutput::Terminate(_) => return Ok(()),
                        ActiveStageOutput::DeactivateAll => {
                            tracing::debug!("Remote desktop reactivation started");
                            send_input(&mut active,&mut image,&mut framed,&input.release()).await?;
                            channels.snapshots.send_modify(|s|{s.frame=None;s.reactivating=true;});
                            let mut sequence=activation_factory.create();
                            let mut buf=ironrdp::core::WriteBuf::new();
                            let reactivation=limited(config.timeout,ErrorStage::Protocol,async {
                                loop {
                                    ironrdp_tokio::single_sequence_step(&mut framed,&mut sequence,&mut buf).await?;
                                    tracing::debug!(state=?sequence.connection_activation_state(),"Remote desktop activation step");
                                    if let connector::connection_activation::ConnectionActivationState::Finalized {desktop_size,share_id,enable_server_pointer,..}=sequence.connection_activation_state() {
                                        break Ok::<_,connector::ConnectorError>((desktop_size,share_id,enable_server_pointer));
                                    }
                                }
                            });
                            let (desktop_size,share_id,enable_server_pointer)=tokio::select! { _=cancellation.cancelled()=>return Ok(()), result=reactivation=>result? };
                            tracing::debug!(width=desktop_size.width,height=desktop_size.height,"Remote desktop reactivation completed");
                            validate_size(desktop_size.width,desktop_size.height)?;
                            image=DecodedImage::new(ironrdp::graphics::image_processing::PixelFormat::BgrA32,desktop_size.width,desktop_size.height);
                            active.set_share_id(share_id);active.set_enable_server_pointer(enable_server_pointer);
                            active.set_fastpath_processor(ironrdp::session::fast_path::ProcessorBuilder {
                                io_channel_id:activation_factory.io_channel_id(),user_channel_id:activation_factory.user_channel_id(),share_id,
                                enable_server_pointer,pointer_software_rendering:false,bulk_decompressor:None,
                            }.build());
                            received_graphics=false;graphics_started=Instant::now();publisher.wait_for_graphics();

                        }
                        _ => {},
                    }
                }
            }
        }
    }
}

async fn send_input(
    active: &mut ironrdp::session::ActiveStage,
    image: &mut DecodedImage,
    framed: &mut TokioFramed<tokio_rustls::client::TlsStream<TcpStream>>,
    events: &[ironrdp::pdu::input::fast_path::FastPathInputEvent],
) -> Result<(), SessionError> {
    for batch in events.chunks(64) {
        for output in active
            .process_fastpath_input(image, batch)
            .map_err(|e| error(ErrorStage::Input, e))?
        {
            if let ActiveStageOutput::ResponseFrame(bytes) = output {
                limited(
                    Duration::from_secs(2),
                    ErrorStage::Network,
                    framed.write_all(&bytes),
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn pump_clipboard(
    clipboard: &crate::clipboard::SharedClipboard,
    active: &mut ironrdp::session::ActiveStage,
    framed: &mut TokioFramed<tokio_rustls::client::TlsStream<TcpStream>>,
    channels: &mut SessionChannels,
) -> Result<(), SessionError> {
    let (initialize, replies, completed, ready, failure) = {
        let mut state = clipboard.lock().unwrap();
        state.tick();
        (
            std::mem::take(&mut state.initialize),
            state.replies(),
            state.completed.take(),
            state.ready,
            state.failure.take(),
        )
    };
    if ready && !channels.snapshots.borrow().capabilities.clipboard_text {
        channels
            .snapshots
            .send_modify(|s| s.capabilities.clipboard_text = true);
    }
    if let Some(result) = completed {
        channels
            .snapshots
            .send_modify(|s| s.clipboard = Some(result));
    }
    if let Some(e) = failure {
        channels
            .snapshots
            .send_modify(|s| s.clipboard = Some((0, Err(e))));
    }
    let mut batches = Vec::new();
    if let Some(channel) = active.get_svc_processor_mut::<ironrdp::cliprdr::CliprdrClient>() {
        if initialize {
            batches.push(
                channel
                    .initiate_copy(&[])
                    .map_err(|e| error(ErrorStage::Clipboard, e))?,
            );
        }
        for reply in replies {
            batches.push(
                channel
                    .submit_format_data(reply)
                    .map_err(|e| error(ErrorStage::Clipboard, e))?,
            );
        }
    }
    for messages in batches {
        let bytes = active
            .process_svc_processor_messages(messages)
            .map_err(|e| error(ErrorStage::Clipboard, e))?;
        limited(
            Duration::from_secs(2),
            ErrorStage::Network,
            framed.write_all(&bytes),
        )
        .await?;
    }
    Ok(())
}
