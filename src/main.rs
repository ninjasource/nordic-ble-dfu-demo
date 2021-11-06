#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(alloc_error_handler)]
//#![allow(unused_imports)]
#![allow(dead_code)]
#![macro_use]

extern crate alloc;
use alloc_cortex_m::CortexMHeap;
use core::{
    alloc::Layout,
    mem,
    sync::atomic::{AtomicUsize, Ordering},
};
use cortex_m::asm::delay;
use cortex_m_rt::entry;
use defmt::{panic, *};
use embassy::{
    executor::Executor,
    util::Forever,
};
use embassy_nrf::interrupt::Priority;
use nrf52840_hal::{self as _};
use nrf_softdevice::{
    ble::{
        gatt_server,
        peripheral::{self, AdvertiseError},
        Connection, TxPower,
    },
    raw, Softdevice,
};
use nrf_softdevice_defmt_rtt as _; // global logger
use panic_probe as _;
use raw::{
    ble_gap_conn_sec_mode_t, sd_ble_gap_device_name_set, sd_power_gpregret_clr,
    sd_power_gpregret_set,
};

static EXECUTOR: Forever<Executor> = Forever::new();

// this is the allocator the application will use
#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();
const HEAP_SIZE: usize = 32 * 1024; // in bytes

// define what happens in an Out Of Memory (OOM) condition
#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    panic!("Alloc error");
}

defmt::timestamp! {"{=u64}", {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        // NOTE(no-CAS) `timestamps` runs with interrupts disabled
        let n = COUNT.load(Ordering::Relaxed);
        COUNT.store(n + 1, Ordering::Relaxed);
        n as u64
    }
}

#[nrf_softdevice::gatt_server]
struct Server {
    dfu: DfuService,
}

#[nrf_softdevice::gatt_service(uuid = "fe59")]
struct DfuService {
    #[characteristic(uuid = "8ec90003-f315-4f60-9fb8-838830daea50", write, notify, indicate)]
    dfu: heapless::Vec<u8, 16>,
}

#[embassy::task]
async fn softdevice_task(sd: &'static Softdevice) {
    sd.run().await;
}

// The advertising data below arbitrarily consists of 3 records (one for each line - called AD Structures) in the following format:
async fn send_bluetooth_adverts(sd: &'static Softdevice) -> Result<Connection, AdvertiseError> {
    // length (1 byte), type (1 byte), data (length - 1 byte(s))
    // where type can be found in the following table: https://www.bluetooth.com/specifications/assigned-numbers/generic-access-profile
    // and length does not include itself. The raw data corresponds to the Payload section of the PDU (protocol data unit) of a BLE advertising packet.
    // The total number of bytes in the PDU Payload is limited to 37 bytes for advertising channel PDUs
    // and about 246 bytes for data PDUs (it depends e.g. extended adverts).
    // See BLUETOOTH CORE SPECIFICATION Version 5.2 | Vol 3, Part C, Chapter 11 for the official spec.
    // See Rubble's ad_structure.rs module for more examples. Scan data follows the same format.
    //  --------------------------------------------------------------------------------------------------
    // | Length        | Type                       | Data                                                |
    // |--------------------------------------------------------------------------------------------------|
    // |      2 (0x02) | Flags (0x01)               | BLE_GAP_ADV_FLAGS_LE_ONLY_GENERAL_DISC_MODE (0x06)  |
    // |      3 (0x03) | GATT Service (0x03)        | Binary Sensor (0x183B big endian byte order)        |
    // |      3 (0x03) | Appearance (0x19)          | Contact Sensor (0x0548 big endian byte order)       |
    // |      5 (0x05) | Complete Local Name (0x09) | Dave (4 characters)                     |
    //  --------------------------------------------------------------------------------------------------
    #[rustfmt::skip]
    let adv_data = &[
        0x02, 0x01, raw::BLE_GAP_ADV_FLAGS_LE_ONLY_GENERAL_DISC_MODE as u8,
        0x03, 0x03, 0x3b, 0x18, 
        0x03, 0x19, 0x48, 0x05, 
        0x05, 0x09, b'D', b'a', b'v', b'e'
    ];

    // Scan data follows the same format as above
    #[rustfmt::skip]
    let scan_data = &[
        0x03, 0x03, 0x3b, 0x18, 
    ];

    let config = peripheral::Config {
        timeout: None,
        tx_power: TxPower::ZerodBm,
        interval: 672, // every 420ms
        ..Default::default()
    };

    let adv = peripheral::ConnectableAdvertisement::ScannableUndirected {
        adv_data,
        scan_data,
    };

    peripheral::advertise_connectable(sd, adv, &config).await
}

async fn connect_bluetooth(conn: Connection, server: &Server) {
    info!("connected");

    let res = gatt_server::run(&conn, server, |e| match e {
        ServerEvent::Dfu(DfuServiceEvent::DfuWrite(val)) => {
            info!("wrote dfu instruction: {}", &val[..]);

            const DFU_OP_RESPONSE_CODE: u8 = 0x20;
            const DFU_OP_ENTER_BOOTLOADER: u8 = 0x01;
            const DFU_OP_SET_ADV_NAME: u8 = 0x02;
            const DFU_RSP_SUCCESS: u8 = 0x01;
            const _DFU_RSP_BUSY: u8 = 0x06;
            const _DFU_RSP_OPERATION_FAILED: u8 = 0x04;
            const DFU_RSP_OP_CODE_NOT_SUPPORTED: u8 = 0x02;

            match val[0] {
                DFU_OP_ENTER_BOOTLOADER => {
                    // enter DFU mode
                    unsafe {
                        sd_power_gpregret_clr(0, 0);
                    }

                    let gpregret_mask = (0xB0 | 0x01) as u32;
                    unsafe {
                        sd_power_gpregret_set(0, gpregret_mask);
                    }

                    let mut resp: heapless::Vec<u8, 16> = heapless::Vec::new();
                    resp.push(DFU_OP_RESPONSE_CODE).unwrap();
                    resp.push(DFU_OP_ENTER_BOOTLOADER).unwrap();
                    resp.push(DFU_RSP_SUCCESS).unwrap();

                    // NOTE that indications are not yet supported but we need one for the nrf connect python app to work
                    if let Err(e) = server.dfu.dfu_notify(&conn, resp) {
                        info!("send notification error: {:?}", e);
                    }

                    delay(80000000); // not sure if this is required (1 second delay)

                    info!("gpregret_mask set. Soft resetting defice...");
                    cortex_m::peripheral::SCB::sys_reset();
                }
                DFU_OP_SET_ADV_NAME => {
                    // change advertisement name
                    // Security Mode 1 Level 1: No security is needed (aka open link).
                    let write_perm = ble_gap_conn_sec_mode_t::new_bitfield_1(1, 1);
                    let len = val[1] as usize;
                    let dev_name = &val[2..len + 2];

                    info!(
                        "setting adv name to {}",
                        core::str::from_utf8(dev_name).unwrap()
                    );

                    unsafe {
                        sd_ble_gap_device_name_set(
                            &write_perm as *const _ as *const ble_gap_conn_sec_mode_t,
                            dev_name as *const _ as *const u8,
                            len as u16,
                        );
                    }

                    let mut resp: heapless::Vec<u8, 16> = heapless::Vec::new();
                    resp.push(DFU_OP_RESPONSE_CODE).unwrap();
                    resp.push(DFU_OP_SET_ADV_NAME).unwrap();
                    resp.push(DFU_RSP_OP_CODE_NOT_SUPPORTED).unwrap();

                    if let Err(e) = server.dfu.dfu_set(resp) {
                        info!("set error: {:?}", e);
                    }

                    let mut resp: heapless::Vec<u8, 16> = heapless::Vec::new();
                    resp.push(DFU_OP_RESPONSE_CODE).unwrap();
                    resp.push(DFU_OP_SET_ADV_NAME).unwrap();
                    resp.push(DFU_RSP_OP_CODE_NOT_SUPPORTED).unwrap();

                    if let Err(e) = server.dfu.dfu_notify(&conn, resp) {
                        info!("send notification error: {:?}", e);
                    }

                    info!("adv name set successfully");
                }
                _ => {
                    // TODO: send error response
                }
            }
        }
        _ => {} // ignore others
    })
    .await;

    if let Err(e) = res {
        info!("gatt_server run exited with error: {:?}", e);
    }
}

#[embassy::task]
async fn bluetooth_task(sd: &'static Softdevice) {
    info!("Bluetooth task");
    let server: Server = unwrap!(gatt_server::register(sd));

    loop {
        info!("Sending adverts");

        match send_bluetooth_adverts(sd).await {
            Ok(connection) => connect_bluetooth(connection, &server).await,
            Err(AdvertiseError::Timeout) => {
                // ignore
            }
            Err(e) => error!("send_bluetooth_adverts: {:?}", e),
        }
    }
}

// NOTE: We cannot use #[embassy::main] here because the Softdevice needs to be enabled before we fetch the peripherals
#[entry]
fn main() -> ! {
    unsafe { ALLOCATOR.init(cortex_m_rt::heap_start() as usize, HEAP_SIZE) }
    info!("Welcome Peripheral");

    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = Priority::P2;
    config.time_interrupt_priority = Priority::P2;

    let config = nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_XTAL as u8,
            rc_ctiv: 0,
            rc_temp_ctiv: 0,
            accuracy: 7,
        }),
        conn_gap: Some(raw::ble_gap_conn_cfg_t {
            conn_count: 6,
            event_length: 24,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 256 }),
        gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
            attr_tab_size: 32768,
        }),
        gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: 3,
            central_role_count: 3,
            central_sec_count: 0,
            _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        gap_device_name: Some(raw::ble_gap_cfg_device_name_t {
            p_value: b"Dave" as *const u8 as _,
            current_len: 4,
            max_len: 4,
            write_perm: unsafe { mem::zeroed() },
            _bitfield_1: raw::ble_gap_cfg_device_name_t::new_bitfield_1(
                raw::BLE_GATTS_VLOC_STACK as u8,
            ),
        }),
        ..Default::default()
    };

    let sd = Softdevice::enable(&config);
    let executor = EXECUTOR.put(Executor::new());

    executor.run(|spawner| {
        unwrap!(spawner.spawn(softdevice_task(sd)));
        unwrap!(spawner.spawn(bluetooth_task(sd)));
    });
}
