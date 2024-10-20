use log::error;
use log::info;

use embassy_futures::join::join3;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use trouble_host::prelude::*;

/// Size of L2CAP packets (ATT MTU is this - 4)
const L2CAP_MTU: usize = 251;

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels.
const L2CAP_CHANNELS_MAX: usize = 2; // Signal + att

const MAX_ATTRIBUTES: usize = 32;

type Resources<C> = HostResources<C, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX, L2CAP_MTU>;

// GATT Server definition
#[gatt_server(attribute_data_size = 32)]
struct Server {
    battery_service: BatteryService,
    my_service: MyService
}

// Battery service
#[gatt_service(uuid = "180f")]
struct BatteryService {
    #[characteristic(uuid = "2a19", read, notify)]
    level: u8,
}

#[gatt_service(uuid = "6D69636861656C73206D616E73696F6E")]
struct MyService {
    #[characteristic(uuid = "6D69636861656C73206D616E73696F6E", read, write_without_response)]
    byte: u8,
}

pub async fn run<C>(controller: C)
where
    C: Controller,
{
    let address = Address::random([0xff, 0x9f, 0x1a, 0x05, 0xe4, 0xff]);
    info!("Our address = {:?}", address);

    let mut resources = Resources::new(PacketQos::None);
    let (stack, peripheral, _, runner) = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .build();

    let mut table: AttributeTable<'_, NoopRawMutex, MAX_ATTRIBUTES> = AttributeTable::new();

    // Generic Access Service (mandatory)
    let id = b"Pico W Bluetooth";
    let appearance = [0x80, 0x07];
    let mut svc = table.add_service(Service::new(0x1800));
    let _ = svc.add_characteristic_ro(0x2a00, id);
    let _ = svc.add_characteristic_ro(0x2a01, &appearance[..]);
    svc.build();

    // Generic attribute service (mandatory)
    table.add_service(Service::new(0x1801));

    let server = Server::new(stack, &mut table);

    info!("Starting advertising and GATT service");
    let _ = join3(
        ble_task(runner),
        gatt_task(&server),
        advertise_task(peripheral, &server),
    )
    .await;
}

async fn ble_task<C: Controller>(mut runner: Runner<'_, C>) -> Result<(), BleHostError<C::Error>> {
    runner.run().await
}

async fn gatt_task<C: Controller>(server: &Server<'_, '_, C>) {
    loop {
        match server.next().await {
            Ok(GattEvent::Write { handle, connection: _ }) => {
                info!("[gatt] pre write event on {:?}", handle);

                let e = server.get(handle, |value| {
                    if handle == server.my_service.byte {
                        info!("[gatt] write on michaels mansion, value {value:?}");
                    } else {
                        info!("[gatt] Write event on {:?}", handle);
                    }
                });

                if let Err(e) = e {
                    error!("[gatt] error on write event {e:?}");
                }
            }
            Ok(GattEvent::Read { handle, connection: _ }) => {
                if handle == server.my_service.byte {
                    server.get(handle, |value| {
                        info!("[gatt] read on michaels mansion; value: {value:?}");
                    }).unwrap();
                } else {
                    info!("[gatt] Read event on {:?}", handle);
                }
            }
            Err(e) => {
                error!("[gatt] Error processing GATT events: {:?}", e);
            }
        }
    }
}

async fn advertise_task<C: Controller>(
    mut peripheral: Peripheral<'_, C>,
    server: &Server<'_, '_, C>,
) -> Result<(), BleHostError<C::Error>> {
    let mut adv_data = [0; 31];
    AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids16(&[Uuid::Uuid16([0x0f, 0x18])]),
            AdStructure::CompleteLocalName(b"Trouble"),
        ],
        &mut adv_data[..],
    )?;
    loop {
        info!("[adv] advertising");
        let mut advertiser = peripheral
            .advertise(
                &Default::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_data[..],
                    scan_data: &[],
                },
            )
            .await?;
        let conn = advertiser.accept().await?;
        info!("[adv] connection established");
        // Keep connection alive
        let mut tick: u8 = 0;
        while conn.is_connected() {
            Timer::after(Duration::from_secs(2)).await;
            tick = tick.wrapping_add(1);
            info!("[adv] notifying connection of tick {}", tick);
            let _ = server.notify(server.battery_service.level, &conn, &[tick]).await;
        }
    }
}
