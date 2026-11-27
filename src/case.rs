//! BLE link to the Smart Case.

use std::fmt;
use std::time::Duration;

use bluer::gatt::remote::Characteristic;
use bluer::gatt::{CharacteristicReader, CharacteristicWriter};
use bluer::{Device, Session, Uuid};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use tokio::time::{sleep, timeout};

use crate::osp::{ErrorCode, Frame, FrameError, Function};

const OSP_SERVICE: Uuid = Uuid::from_u128(0x0000fcbc_0000_1000_8000_00805f9b34fb);
const OSP_CHARACTERISTIC: Uuid = Uuid::from_u128(0x1732c22e_048b_11ee_be56_0242ac120002);
const FIRMWARE_UPDATE_CHARACTERISTIC: Uuid =
    Uuid::from_u128(0x1732c346_048b_11ee_be56_0242ac120002);

const LE_BEARER: &str = "org.bluez.Bearer.LE1";
const LE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DISCOVERY_ATTEMPTS: u32 = 20;
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

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
                "no Sleepbuds case is known to Bluetooth. Pair it for audio first"
            ),
            Self::NotBonded => write!(
                f,
                "this computer is not bonded with the case. Run `ozzzlo pair`"
            ),
            Self::LinkBusy => write!(f, "another program is already talking to the case"),
            Self::Disconnected => write!(
                f,
                "the case is not reachable over Bluetooth LE. Open its lid"
            ),
            Self::NoReply(function) => write!(f, "the case did not answer {function}"),
            Self::Refused(function, code) => write!(f, "the case refused {function}: {code}"),
            Self::Hazardous(function) => write!(f, "refusing to send {function}: it is hazardous"),
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
}

impl Case {
    /// Opens the OSP channel to the first known device that offers the OSP service.
    ///
    /// Brings up the LE link if it is down, without touching the audio link.
    pub async fn connect() -> Result<Self, Error> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        let mut case = None;
        for address in adapter.device_addresses().await? {
            let device = adapter.device(address)?;
            let offers_osp = device
                .uuids()
                .await?
                .is_some_and(|uuids| uuids.contains(&OSP_SERVICE));
            if offers_osp {
                case = Some(device);
                break;
            }
        }
        let device = case.ok_or(Error::NoCase)?;
        let object_path = format!(
            "/org/bluez/{}/dev_{}",
            adapter.name(),
            device.address().to_string().replace(':', "_")
        );

        let bond_path = object_path.clone();
        let bonded = tokio::task::spawn_blocking(move || le_bond(&bond_path))
            .await
            .unwrap_or(None);
        if bonded == Some(false) {
            return Err(Error::NotBonded);
        }

        if characteristic(&device, OSP_CHARACTERISTIC).await.is_none() {
            log::info!("connecting to {} over LE", device.address());
            let connected = tokio::task::spawn_blocking(move || le_connect(&object_path)).await;
            if !matches!(connected, Ok(Ok(()))) {
                log::warn!("LE-only connect failed ({connected:?}), using Device1.Connect");
                device.connect().await.map_err(|error| {
                    log::warn!("connect failed: {error}");
                    Error::Disconnected
                })?;
            }
        }
        let mut osp = None;
        for _ in 0..DISCOVERY_ATTEMPTS {
            osp = characteristic(&device, OSP_CHARACTERISTIC).await;
            if osp.is_some() {
                break;
            }
            sleep(DISCOVERY_INTERVAL).await;
        }
        let osp = osp.ok_or(Error::Disconnected)?;
        let firmware_update = characteristic(&device, FIRMWARE_UPDATE_CHARACTERISTIC)
            .await
            .ok_or(Error::Disconnected)?;

        let case = Self {
            firmware_update: subscription_to(&firmware_update).await?,
            replies: subscription_to(&osp).await?,
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
    pub async fn recv(&self) -> Result<Frame, Error> {
        loop {
            let notification = tokio::select! {
                bytes = self.replies.recv() => bytes,
                bytes = self.firmware_update.recv() => bytes,
            };
            let bytes = match notification {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => return Err(Error::Disconnected),
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

    /// Sends `request` and returns the frame that answers it.
    ///
    /// `seen` receives every frame that arrives meanwhile, the answer included.
    /// An `Error` answer and a silent case both fail.
    pub async fn exchange(
        &self,
        request: &Frame,
        mut seen: impl FnMut(&Frame),
    ) -> Result<Frame, Error> {
        self.send(request).await?;
        loop {
            let frame = timeout(REPLY_TIMEOUT, self.recv())
                .await
                .map_err(|_| Error::NoReply(request.function))??;
            seen(&frame);
            if frame.answers(request) {
                return match frame.refusal() {
                    Some(code) => Err(Error::Refused(frame.function, code)),
                    None => Ok(frame),
                };
            }
        }
    }
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
    characteristic.notify_io().await.map_err(|error| match error.kind {
        bluer::ErrorKind::NotPermitted => Error::LinkBusy,
        _ => Error::Bluetooth(error),
    })
}

/// Whether BlueZ holds an LE bond for the device, or `None` if this BlueZ
/// version does not expose bearers.
fn le_bond(object_path: &str) -> Option<bool> {
    let bus = Connection::new_system().ok()?;
    bus.with_proxy("org.bluez", object_path, Duration::from_secs(5))
        .get(LE_BEARER, "Bonded")
        .ok()
}

fn le_connect(object_path: &str) -> Result<(), dbus::Error> {
    let bus = Connection::new_system()?;
    bus.with_proxy("org.bluez", object_path, LE_CONNECT_TIMEOUT)
        .method_call(LE_BEARER, "Connect", ())
}
