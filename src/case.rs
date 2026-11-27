//! BLE connection to the Smart Case.

use std::time::Duration;

use bluer::gatt::remote::Characteristic;
use bluer::gatt::{CharacteristicReader, CharacteristicWriter};
use bluer::{Device, Session, Uuid};
use tokio::time::{sleep, timeout};

use crate::osp::{Frame, Operator};

const OSP_SERVICE: Uuid = Uuid::from_u128(0x0000fcbc_0000_1000_8000_00805f9b34fb);
const OSP_CHARACTERISTIC: Uuid = Uuid::from_u128(0x1732c22e_048b_11ee_be56_0242ac120002);
const FIRMWARE_UPDATE_CHARACTERISTIC: Uuid =
    Uuid::from_u128(0x1732c346_048b_11ee_be56_0242ac120002);

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const SERVICE_DISCOVERY_ATTEMPTS: u32 = 20;

/// Errors from any layer between the CLI and the case.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// An open OSP channel to a bonded Smart Case.
pub struct Case {
    writer: CharacteristicWriter,
    reader: CharacteristicReader,
    // The case answers on the OSP characteristic only while this one is subscribed too.
    _firmware_update: CharacteristicReader,
}

impl Case {
    /// Opens the OSP channel to the first known device that offers the OSP service.
    ///
    /// The case MUST already hold an LE bond with this host.
    pub async fn connect() -> Result<Self> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        for address in adapter.device_addresses().await? {
            let device = adapter.device(address)?;
            let offers_osp = device
                .uuids()
                .await?
                .is_some_and(|uuids| uuids.contains(&OSP_SERVICE));
            if offers_osp {
                let osp = characteristic(&device, OSP_CHARACTERISTIC).await?;
                let firmware_update =
                    characteristic(&device, FIRMWARE_UPDATE_CHARACTERISTIC).await?;
                return Ok(Self {
                    _firmware_update: firmware_update.notify_io().await?,
                    reader: osp.notify_io().await?,
                    writer: osp.write_io().await?,
                });
            }
        }
        Err("no known Bluetooth device offers the OSP service".into())
    }

    /// Sends `request` and returns the case's reply for the same function.
    ///
    /// Notifications about other functions are skipped. An `Error` reply and
    /// a missing reply both fail.
    pub async fn request(&self, request: &Frame) -> Result<Frame> {
        let mut envelope = vec![0x00];
        envelope.extend(request.encode()?);
        self.writer.send(&envelope).await?;

        let reply = timeout(REPLY_TIMEOUT, async {
            loop {
                let notification = self.reader.recv().await?;
                let frame = Frame::decode(notification.get(1..).unwrap_or_default())?;
                if frame.function == request.function {
                    return Result::Ok(frame);
                }
            }
        })
        .await
        .map_err(|_| "the case did not reply")??;

        if reply.operator == Operator::Error {
            return Err(format!("the case reported error {:02x?}", reply.payload).into());
        }
        Ok(reply)
    }
}

async fn characteristic(device: &Device, uuid: Uuid) -> Result<Characteristic> {
    for attempt in 0..SERVICE_DISCOVERY_ATTEMPTS {
        for service in device.services().await? {
            if service.uuid().await? == OSP_SERVICE {
                for characteristic in service.characteristics().await? {
                    if characteristic.uuid().await? == uuid {
                        return Ok(characteristic);
                    }
                }
            }
        }
        if attempt == 0 {
            device.connect().await?;
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err("the case is not reachable over Bluetooth LE".into())
}
