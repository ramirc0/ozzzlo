//! BLE link to the Smart Case.

use std::fmt;
use std::time::Duration;

use bluer::gatt::remote::Characteristic;
use bluer::gatt::{CharacteristicReader, CharacteristicWriter};
use bluer::l2cap::{Security, SecurityLevel, SeqPacket, Socket, SocketAddr};
use bluer::{Address, AddressType, Device, Session, Uuid};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use tokio::time::{sleep, timeout};

use crate::osp::{ErrorCode, Frame, FrameError, Function};

const OSP_SERVICE: Uuid = Uuid::from_u128(0x0000fcbc_0000_1000_8000_00805f9b34fb);
const OSP_CHARACTERISTIC: Uuid = Uuid::from_u128(0x1732c22e_048b_11ee_be56_0242ac120002);
const DEVICE_INFORMATION_CHARACTERISTIC: Uuid =
    Uuid::from_u128(0x1732b6a8_048b_11ee_be56_0242ac120002);
const FIRMWARE_UPDATE_CHARACTERISTIC: Uuid =
    Uuid::from_u128(0x1732c346_048b_11ee_be56_0242ac120002);

const LE_BEARER: &str = "org.bluez.Bearer.LE1";
const AUDIO_BEARER: &str = "org.bluez.Bearer.BREDR1";
// A reachable case links in about two seconds.
const LE_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const DISCOVERY_ATTEMPTS: u32 = 20;
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
const PAIRING_MODE_POLLS: u32 = 120;
const PAIRING_MODE_OFF: Function = Function { block: 4, id: 8 };

/// Why the case could not be reached or did not do what was asked.
#[derive(Debug)]
pub enum Error {
    /// BlueZ knows no device that offers the OSP service.
    NoCase,
    /// This host holds no LE bond with the case.
    NotBonded,
    /// Another program holds the case's notifications.
    LinkBusy,
    /// The LE link dropped or could not be brought up.
    Disconnected,
    /// The case stayed silent after a request.
    NoReply(Function),
    /// The case answered a request with an error code.
    Refused(Function, ErrorCode),
    /// The frame targets a function that could damage the case.
    Hazardous(Function),
    /// The case never entered pairing mode.
    NotInPairingMode,
    /// A frame could not be encoded.
    Frame(FrameError),
    /// BlueZ reported a failure.
    Bluetooth(bluer::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCase => write!(
                f,
                "this computer has never seen a Sleepbuds case. Hold the case button for \
                 3 seconds with the lid open, pair it in your Bluetooth settings, then try again"
            ),
            Self::NotBonded => write!(
                f,
                "this computer is not bonded with the case. Run `ozzzlo pair`"
            ),
            Self::LinkBusy => write!(f, "another program is already talking to the case"),
            Self::Disconnected => write!(
                f,
                "the case is not reachable over Bluetooth LE. \
                 It may be out of range or asleep, or the phone app may be connected to it"
            ),
            Self::NoReply(function) => write!(f, "the case did not answer {function}"),
            Self::Refused(function, code) => write!(f, "the case refused {function}: {code}"),
            Self::Hazardous(function) => write!(f, "refusing to send {function}: it is hazardous"),
            Self::NotInPairingMode => write!(
                f,
                "the case did not pair in time. Hold its button for 3 seconds with the lid open \
                 and run `ozzzlo pair` again"
            ),
            Self::Frame(error) => write!(f, "{error}"),
            Self::Bluetooth(error) => write!(f, "Bluetooth: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<bluer::Error> for Error {
    fn from(error: bluer::Error) -> Self {
        Self::Bluetooth(error)
    }
}

impl From<FrameError> for Error {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

/// An open OSP channel to a bonded Smart Case.
pub struct Case {
    // Notifications stop once the session that created the readers is dropped.
    _session: Session,
    // An unanswered write request wedges BlueZ's WriteValue with InProgress
    // until bluetoothd restarts. The acquired writer cannot wedge.
    writer: CharacteristicWriter,
    replies: CharacteristicReader,
    // The case answers on the OSP characteristic only while this one is subscribed too.
    firmware_update: CharacteristicReader,
    address: Address,
}

/// The case's L2CAP channel for bulk transfers. Frames use the BLE envelope.
pub struct TransferChannel {
    socket: SeqPacket,
}

impl TransferChannel {
    /// Writes `frame` to the channel.
    ///
    /// # Errors
    ///
    /// Fails with `Hazardous` for a dangerous function and `Disconnected`
    /// when the write fails.
    pub async fn send(&self, frame: &Frame) -> Result<(), Error> {
        self.write(frame, frame.encode()?).await
    }

    /// Writes `frame` to the channel with a two-byte length, the form the
    /// case expects for file data.
    ///
    /// # Errors
    ///
    /// Fails as [`TransferChannel::send`] does.
    pub async fn send_long(&self, frame: &Frame) -> Result<(), Error> {
        self.write(frame, frame.encode_long()?).await
    }

    async fn write(&self, frame: &Frame, encoded: Vec<u8>) -> Result<(), Error> {
        if frame.function.is_hazardous() {
            return Err(Error::Hazardous(frame.function));
        }
        let mut envelope = vec![0x00];
        envelope.extend(encoded);
        log::debug!("=> {frame}  {} {:?}", frame.function, frame.operator);
        self.socket.send(&envelope).await.map_err(|error| {
            log::warn!("channel write of {} failed: {error}", frame.function);
            Error::Disconnected
        })?;
        Ok(())
    }

    /// Waits for the next frame the case sends on the channel.
    ///
    /// # Errors
    ///
    /// Fails with `Disconnected` when the channel closes.
    pub async fn recv(&self) -> Result<Frame, Error> {
        loop {
            let mut packet = vec![0; self.socket.recv_mtu().unwrap_or(1024)];
            let length = self.socket.recv(&mut packet).await.map_err(|error| {
                log::warn!("channel read failed: {error}");
                Error::Disconnected
            })?;
            if length == 0 {
                log::warn!("the case closed the transfer channel");
                return Err(Error::Disconnected);
            }
            log::trace!("<= raw {:02x?}", &packet[..length]);
            match Frame::decode(&packet[1..length]) {
                Ok(frame) => {
                    log::debug!("<= {frame}  {} {:?}", frame.function, frame.operator);
                    return Ok(frame);
                }
                Err(error) => log::warn!("undecodable channel packet: {error}"),
            }
        }
    }
}

impl Case {
    /// Opens the OSP channel to the first known device that offers the OSP service.
    ///
    /// Brings up the LE link if it is down, without touching the audio link.
    ///
    /// # Errors
    ///
    /// Fails with `NoCase`, `NotBonded`, `LinkBusy` or `Disconnected` when the case
    /// cannot be reached, and with `Bluetooth` when BlueZ reports a failure.
    pub async fn connect() -> Result<Self, Error> {
        let session = Session::new().await?;
        let (device, object_path) = known_case(&session).await?;

        let bearer_path = object_path.clone();
        let bearer = tokio::task::spawn_blocking(move || le_bearer(&bearer_path)).await;
        let linked = match bearer {
            Ok(Some(LeBearer { bonded: false, .. })) => return Err(Error::NotBonded),
            Ok(Some(LeBearer { connected, .. })) => connected,
            // BlueZ without bearer support cannot tell an LE link from an audio link.
            _ => false,
        };
        ensure_le_link(&device, object_path, linked).await?;
        let osp = discovered(&device, OSP_CHARACTERISTIC).await?;
        let firmware_update = characteristic(&device, FIRMWARE_UPDATE_CHARACTERISTIC)
            .await
            .ok_or(Error::Disconnected)?;

        let case = Self {
            firmware_update: subscription_to(&firmware_update).await?,
            replies: subscription_to(&osp).await?,
            address: device.address(),
            writer: osp.write_io().await?,
            _session: session,
        };
        log::info!(
            "connected to {} (mtu {})",
            device.address(),
            case.replies.mtu()
        );
        Ok(case)
    }

    /// Writes `frame` to the case.
    ///
    /// Refuses hazardous functions.
    ///
    /// # Errors
    ///
    /// Fails with `Hazardous` for a dangerous function, `Frame` for an oversized
    /// payload, and `Disconnected` when the write fails.
    pub async fn send(&self, frame: &Frame) -> Result<(), Error> {
        if frame.function.is_hazardous() {
            return Err(Error::Hazardous(frame.function));
        }
        let mut envelope = vec![0x00];
        envelope.extend(frame.encode()?);
        log::debug!("-> {frame}  {} {:?}", frame.function, frame.operator);
        self.writer.send(&envelope).await.map_err(|error| {
            log::warn!("write of {} failed: {error}", frame.function);
            Error::Disconnected
        })
    }

    /// Waits for the next frame the case sends, solicited or not.
    ///
    /// # Errors
    ///
    /// Fails with `Disconnected` when the link drops.
    pub async fn recv(&self) -> Result<Frame, Error> {
        loop {
            let notification = tokio::select! {
                bytes = self.replies.recv() => bytes,
                bytes = self.firmware_update.recv() => bytes,
            };
            let bytes = match notification {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => {
                    log::warn!("the notification stream ended");
                    return Err(Error::Disconnected);
                }
                Err(error) => {
                    log::warn!("notification read failed: {error}");
                    return Err(Error::Disconnected);
                }
            };
            log::trace!("<- raw {bytes:02x?}");
            match Frame::decode(&bytes[1..]) {
                Ok(frame) => {
                    log::debug!("<- {frame}  {} {:?}", frame.function, frame.operator);
                    return Ok(frame);
                }
                Err(error) => log::warn!("undecodable notification {bytes:02x?}: {error}"),
            }
        }
    }

    /// Opens the case's L2CAP transfer channel on `psm`.
    ///
    /// # Errors
    ///
    /// Fails with `Disconnected` when the channel cannot be opened.
    pub async fn open_transfer_channel(&self, psm: u16) -> Result<TransferChannel, Error> {
        let opened = async {
            let socket = Socket::<SeqPacket>::new_seq_packet()?;
            // Without an LE source address, Linux sends on the channel without
            // the length prefix, and the case closes it.
            socket.bind(SocketAddr::any_le())?;
            socket.set_security(Security {
                level: SecurityLevel::Medium,
                key_size: 0,
            })?;
            socket
                .connect(SocketAddr::new(self.address, AddressType::LePublic, psm))
                .await
        };
        match timeout(LE_CONNECT_TIMEOUT, opened).await {
            Ok(Ok(socket)) => {
                log::info!(
                    "transfer channel open on psm {psm} (send mtu {:?}, recv mtu {:?})",
                    socket.send_mtu(),
                    socket.recv_mtu()
                );
                Ok(TransferChannel { socket })
            }
            failed => {
                log::warn!("transfer channel on psm {psm} did not open: {failed:?}");
                Err(Error::Disconnected)
            }
        }
    }

    /// Sends `request` and returns the frame that answers it.
    ///
    /// `seen` receives every frame that arrives meanwhile, the answer included.
    ///
    /// # Errors
    ///
    /// Fails as [`Case::send`] does, with `NoReply` when the case stays silent, and
    /// with `Refused` when it answers with an error code.
    pub async fn exchange(
        &self,
        request: &Frame,
        seen: impl FnMut(&Frame),
    ) -> Result<Frame, Error> {
        self.exchange_until(request, |frame| frame.answers(request), REPLY_TIMEOUT, seen)
            .await
    }

    /// Sends `request` and returns the first frame that `settles` it.
    ///
    /// `patience` is how long the case may stay silent before the wait fails.
    /// `seen` receives every frame that arrives meanwhile.
    ///
    /// # Errors
    ///
    /// Fails as [`Case::send`] does, with `NoReply` when the case stays silent, and
    /// with `Refused` when the settling frame is an error reply.
    pub async fn exchange_until(
        &self,
        request: &Frame,
        settles: impl Fn(&Frame) -> bool,
        patience: Duration,
        mut seen: impl FnMut(&Frame),
    ) -> Result<Frame, Error> {
        self.send(request).await?;
        loop {
            let frame = timeout(patience, self.recv())
                .await
                .map_err(|_| Error::NoReply(request.function))??;
            seen(&frame);
            if settles(&frame) {
                return match frame.refusal() {
                    Some(code) => Err(Error::Refused(frame.function, code)),
                    None => Ok(frame),
                };
            }
        }
    }
}

/// Connects this computer to the buds for audio.
///
/// The case keeps a control link up only for a host that also holds an audio
/// link. Connecting audio here takes it from any other device that had it.
///
/// # Errors
///
/// Fails with `NoCase` when BlueZ knows no case, and with `Disconnected` when
/// the audio link cannot be brought up.
pub async fn connect_audio() -> Result<(), Error> {
    let session = Session::new().await?;
    let (device, object_path) = known_case(&session).await?;
    log::info!("connecting audio to {}", device.address());
    let connected =
        tokio::task::spawn_blocking(move || bearer_call(&object_path, AUDIO_BEARER, "Connect"))
            .await;
    match connected {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error))
            if error
                .name()
                .is_some_and(|name| name.contains("AlreadyConnected")) =>
        {
            Ok(())
        }
        // A BlueZ without bearers connects every profile at once.
        Ok(Err(error)) if error.name().is_some_and(|name| name.contains("Unknown")) => {
            device.connect().await.map_err(|error| {
                log::warn!("audio connect failed: {error}");
                Error::Disconnected
            })
        }
        failed => {
            log::warn!("audio connect failed: {failed:?}");
            Err(Error::Disconnected)
        }
    }
}

/// Bonds this host with the case over LE, which the case allows only in pairing mode.
///
/// Calls `on_progress` with what the user should know or do each time the
/// situation changes, and waits up to two minutes for the case to enter
/// pairing mode. Returns `false` if a bond already existed.
///
/// # Errors
///
/// Fails with `NoCase` or `Disconnected` when the case cannot be reached,
/// `NotInPairingMode` when the wait runs out, and `Bluetooth` when BlueZ
/// rejects the pairing.
pub async fn pair(mut on_progress: impl FnMut(&str)) -> Result<bool, Error> {
    let session = Session::new().await?;
    let (device, object_path) = known_case(&session).await?;
    // Without an agent of our own, the desktop's agent may prompt the user.
    let _agent = session
        .register_agent(bluer::agent::Agent::default())
        .await?;
    let mut last_guidance = "";
    let mut paired = false;
    // Holding the case button drops the LE link, and pairing mode can lapse
    // between the check and the pairing. Every step is retried until the
    // wait runs out.
    for _ in 0..PAIRING_MODE_POLLS {
        let state = pairing_mode(&device, &object_path).await;
        if let Err(error) = &state {
            log::debug!("waiting for the case: {error}");
        }
        if matches!(state, Ok(PairingMode::AlreadyBonded)) {
            return Ok(false);
        }
        let guidance = guidance_for(&state);
        if guidance != last_guidance {
            on_progress(guidance);
            last_guidance = guidance;
        }
        if matches!(state, Ok(PairingMode::On)) {
            log::info!("the case is in pairing mode, pairing");
            match device.pair().await {
                Ok(()) => {
                    paired = true;
                    break;
                }
                Err(error) => log::warn!("pairing attempt failed: {error}"),
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
    if !paired {
        return Err(Error::NotInPairingMode);
    }
    log::info!("paired with {}", device.address());
    drop(session);

    // The vendor app ends pairing mode itself once the bond exists.
    let case = Case::connect().await?;
    let mut leave_pairing_mode = Frame::start(PAIRING_MODE_OFF);
    leave_pairing_mode.payload = vec![0];
    case.send(&leave_pairing_mode).await?;
    Ok(true)
}

/// What to tell the user for each thing a pairing-mode check can find.
fn guidance_for(state: &Result<PairingMode, Error>) -> &'static str {
    match state {
        Ok(PairingMode::On) => "The case is in pairing mode. Pairing now.",
        Ok(PairingMode::Off) => {
            "The case is not in pairing mode. Hold its button for 3 seconds with the lid open."
        }
        Ok(PairingMode::AlreadyBonded) => "",
        Err(_) => "Looking for the case. Keep its lid open.",
    }
}

enum PairingMode {
    AlreadyBonded,
    On,
    Off,
}

/// Links to the case if needed and reads whether it is in pairing mode.
async fn pairing_mode(device: &Device, object_path: &str) -> Result<PairingMode, Error> {
    let bearer_path = object_path.to_string();
    let bearer = tokio::task::spawn_blocking(move || le_bearer(&bearer_path)).await;
    let linked = match bearer {
        Ok(Some(LeBearer { bonded: true, .. })) => return Ok(PairingMode::AlreadyBonded),
        Ok(Some(LeBearer { connected, .. })) => connected,
        _ => false,
    };
    ensure_le_link(device, object_path.to_string(), linked).await?;
    let information = characteristic(device, DEVICE_INFORMATION_CHARACTERISTIC)
        .await
        .ok_or(Error::Disconnected)?;
    if pairing_mode_is_on(&information.read().await?) {
        Ok(PairingMode::On)
    } else {
        Ok(PairingMode::Off)
    }
}

/// Whether the unencrypted device information blob reports pairing mode.
///
/// The flags are the big-endian `u16` at bytes 17 and 18. Bit 0 is pairing mode.
fn pairing_mode_is_on(information: &[u8]) -> bool {
    information.get(18).is_some_and(|flags| flags & 1 == 1)
}

/// Finds the first device BlueZ knows that offers the OSP service.
async fn known_case(session: &Session) -> Result<(Device, String), Error> {
    let adapter = session.default_adapter().await?;
    for address in adapter.device_addresses().await? {
        let device = adapter.device(address)?;
        let offers_osp = device
            .uuids()
            .await?
            .is_some_and(|uuids| uuids.contains(&OSP_SERVICE));
        if offers_osp {
            let object_path = format!(
                "/org/bluez/{}/dev_{}",
                adapter.name(),
                address.to_string().replace(':', "_")
            );
            return Ok((device, object_path));
        }
    }
    Err(Error::NoCase)
}

/// Brings the LE link to a state where GATT lookups answer.
async fn ensure_le_link(device: &Device, object_path: String, linked: bool) -> Result<(), Error> {
    // BlueZ can report the LE link up while it has forgotten the services,
    // for example after the audio link reconnects. bluer then waits two
    // minutes on every lookup. Cycling the LE link makes BlueZ resolve again.
    if linked && !device.is_services_resolved().await? {
        log::info!("BlueZ lost the case's services, cycling the LE link");
        let path = object_path.clone();
        let dropped =
            tokio::task::spawn_blocking(move || bearer_call(&path, LE_BEARER, "Disconnect")).await;
        log::debug!("LE disconnect: {dropped:?}");
        return link_over_le(device, object_path).await;
    }
    if !linked {
        return link_over_le(device, object_path).await;
    }
    Ok(())
}

async fn link_over_le(device: &Device, object_path: String) -> Result<(), Error> {
    log::info!("connecting to {} over LE", device.address());
    let cancel_path = object_path.clone();
    let connected =
        tokio::task::spawn_blocking(move || bearer_call(&object_path, LE_BEARER, "Connect")).await;
    match connected {
        Ok(Ok(())) => {}
        // Only a BlueZ without LE bearers needs the call that may also raise audio.
        Ok(Err(error)) if error.name().is_some_and(|name| name.contains("Unknown")) => {
            log::info!("BlueZ has no LE bearer ({error}), using Device1.Connect");
            device.connect().await.map_err(|error| {
                log::warn!("connect failed: {error}");
                Error::Disconnected
            })?;
        }
        // BlueZ is already bringing the LE link up, for example right after an
        // audio connect. Wait for that attempt instead of failing or cancelling it.
        Ok(Err(error)) if error.name().is_some_and(|name| name.contains("InProgress")) => {
            log::info!("BlueZ is already connecting over LE, waiting for it");
            for _ in 0..DISCOVERY_ATTEMPTS {
                sleep(DISCOVERY_INTERVAL).await;
                let path = cancel_path.clone();
                let bearer = tokio::task::spawn_blocking(move || le_bearer(&path)).await;
                if matches!(
                    bearer,
                    Ok(Some(LeBearer {
                        connected: true,
                        ..
                    }))
                ) {
                    return Ok(());
                }
            }
            return Err(Error::Disconnected);
        }
        failed => {
            log::info!("LE connect failed: {failed:?}");
            // BlueZ keeps trying after the call times out, and a late success
            // would take the case away from the phone app unasked.
            let cancelled = tokio::task::spawn_blocking(move || {
                bearer_call(&cancel_path, LE_BEARER, "Disconnect")
            })
            .await;
            log::debug!("cancelled the pending LE connect: {cancelled:?}");
            return Err(Error::Disconnected);
        }
    }
    Ok(())
}

/// Waits for GATT discovery to expose the characteristic.
async fn discovered(device: &Device, uuid: Uuid) -> Result<Characteristic, Error> {
    for _ in 0..DISCOVERY_ATTEMPTS {
        if let Some(found) = characteristic(device, uuid).await {
            return Ok(found);
        }
        sleep(DISCOVERY_INTERVAL).await;
    }
    Err(Error::Disconnected)
}

async fn characteristic(device: &Device, uuid: Uuid) -> Option<Characteristic> {
    for service in device.services().await.ok()? {
        if service.uuid().await.ok()? == OSP_SERVICE {
            for characteristic in service.characteristics().await.ok()? {
                if characteristic.uuid().await.ok()? == uuid {
                    return Some(characteristic);
                }
            }
        }
    }
    None
}

async fn subscription_to(characteristic: &Characteristic) -> Result<CharacteristicReader, Error> {
    characteristic
        .notify_io()
        .await
        .map_err(|error| match error.kind {
            bluer::ErrorKind::NotPermitted => Error::LinkBusy,
            // The link can drop between discovery and subscription.
            _ => {
                log::info!("subscribing failed: {error}");
                Error::Disconnected
            }
        })
}

struct LeBearer {
    bonded: bool,
    connected: bool,
}

/// The device's LE bearer state, or `None` if this BlueZ does not expose bearers.
fn le_bearer(object_path: &str) -> Option<LeBearer> {
    let bus = Connection::new_system().ok()?;
    let device = bus.with_proxy("org.bluez", object_path, Duration::from_secs(5));
    Some(LeBearer {
        bonded: device.get(LE_BEARER, "Bonded").ok()?,
        connected: device.get(LE_BEARER, "Connected").ok()?,
    })
}

fn bearer_call(object_path: &str, bearer: &str, method: &str) -> Result<(), dbus::Error> {
    let bus = Connection::new_system()?;
    bus.with_proxy("org.bluez", object_path, LE_CONNECT_TIMEOUT)
        .method_call(bearer, method, ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_is_asked_for_only_when_pairing_mode_is_known_to_be_off() {
        let asks_for_button = |state| guidance_for(&state).contains("Hold its button");
        assert!(asks_for_button(Ok(PairingMode::Off)));
        assert!(!asks_for_button(Ok(PairingMode::On)));
        assert!(!asks_for_button(Err(Error::Disconnected)));
    }

    #[test]
    fn pairing_mode_is_bit_zero_of_the_last_flag_byte() {
        let mut information = [
            0, 3, 26, 66, 1, 0, 122, 5, 5, 0, 0, 0, 0, 45, 0, 47, 9, 0, 6,
        ];
        assert!(!pairing_mode_is_on(&information));
        information[18] = 7;
        assert!(pairing_mode_is_on(&information));
        assert!(!pairing_mode_is_on(&information[..10]));
    }
}
