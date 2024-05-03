# (WIP) Run [rs-matter](https://github.com/project-chip/rs-matter) on Espressif chips with [ESP IDF](https://github.com/esp-rs/esp-idf-svc)

[![CI](https://github.com/ivmarkov/esp-idf-matter/actions/workflows/ci.yml/badge.svg)](https://github.com/ivmarkov/esp-idf-matter/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/esp-idf-matter.svg)](https://crates.io/crates/esp-idf-matter)
[![Documentation](https://img.shields.io/badge/docs-esp--rs-brightgreen)](https://ivmarkov.github.io/esp-idf-matter/esp_idf_matter/index.html)
[![Matrix](https://img.shields.io/matrix/ivmarkov:matrix.org?label=join%20matrix&color=BEC5C9&logo=matrix)](https://matrix.to/#/#esp-rs:matrix.org)
[![Wokwi](https://img.shields.io/endpoint?url=https%3A%2F%2Fwokwi.com%2Fbadge%2Fclick-to-simulate.json)](https://wokwi.com/projects/332188235906155092)

## Overview

Configuring and running the [`rs-matter`](https://github.com/project-chip/rs-matter) crate is not trivial.

Users are expected to provide implementations for various `rs-matter` abstractions, like a UDP stack, BLE stack, randomizer, epoch time, responder and so on and so forth. 

Furthermore, _operating_ the assembled Matter stack is also challenging, as various features might need to be switched on or off depending on whether Matter is running in commissioning or operating mode, and also depending on the current network connectivity (as in e.g. Wifi signal lost).

**This crate addresses these issues by providing an all-in-one [`MatterStack`](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/stack.rs#L57) assembly that configures `rs-matter` for reliably operating on top of the ESP IDF SDK.**

Instantiate it and then call `MatterStack::run(...)`.

```rust
//! An example utilizing the `WifiBleMatterStack` struct.
//! As the name suggests, this Matter stack assembly uses Wifi as the main transport,
//! and BLE for commissioning.
//! If you want to use Ethernet, utilize `EthMatterStack` instead.
//!
//! The example implements a fictitious Light device (an On-Off Matter cluster).

use core::borrow::Borrow;
use core::pin::pin;

use embassy_futures::select::select;
use embassy_time::{Duration, Timer};

use esp_idf_matter::{init_async_io, Error, MdnsType, WifiBleMatterStack};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::EspTaskTimerService;

use log::{error, info};

use rs_matter::data_model::cluster_basic_information::BasicInfoConfig;
use rs_matter::data_model::cluster_on_off;
use rs_matter::data_model::device_types::DEV_TYPE_ON_OFF_LIGHT;
use rs_matter::data_model::objects::{Endpoint, HandlerCompat, Node};
use rs_matter::data_model::system_model::descriptor;
use rs_matter::secure_channel::spake2p::VerifierData;
use rs_matter::utils::select::Coalesce;
use rs_matter::CommissioningData;

use static_cell::ConstStaticCell;

#[path = "dev_att/dev_att.rs"]
mod dev_att;

fn main() -> Result<(), Error> {
    EspLogger::initialize_default();

    info!("Starting...");

    // Run in a higher-prio thread to avoid issues with `async-io` getting
    // confused by the low priority of the ESP IDF main task
    // Also allocate a very large stack (for now) as `rs-matter` futures do occupy quite some space
    let thread = std::thread::Builder::new()
        .stack_size(65 * 1024)
        .spawn(|| {
            // Eagerly initialize `async-io` to minimize the risk of stack blowups later on
            init_async_io()?;

            run()
        })
        .unwrap();

    thread.join().unwrap()
}

#[inline(never)]
#[cold]
fn run() -> Result<(), Error> {
    let result = block_on(matter());

    if let Err(e) = &result {
        error!("Matter aborted execution with error: {:?}", e);
    }
    {
        info!("Matter finished execution successfully");
    }

    result
}

async fn matter() -> Result<(), Error> {
    // Take the Matter stack (can be done only once),
    // as we'll run it in this thread
    let stack = MATTER_STACK.take();

    // Our "light" on-off cluster.
    // Can be anything implementing `rs_matter::data_model::AsyncHandler`
    let on_off = cluster_on_off::OnOffCluster::new(*stack.matter().borrow());

    // Chain our endpoint clusters with the
    // (root) Endpoint 0 system clusters in the final handler
    let handler = stack
        .root_handler()
        // Our on-off cluster, on Endpoint 1
        .chain(
            LIGHT_ENDPOINT_ID,
            cluster_on_off::ID,
            HandlerCompat(&on_off),
        )
        // Each Endpoint needs a Descriptor cluster too
        // Just use the one that `rs-matter` provides out of the box
        .chain(
            LIGHT_ENDPOINT_ID,
            descriptor::ID,
            HandlerCompat(descriptor::DescriptorCluster::new(*stack.matter().borrow())),
        );

    // Run the Matter stack with our handler
    // Using `pin!` is completely optional, but saves some memory due to `rustc`
    // not being very intelligent w.r.t. stack usage in async functions
    let mut matter = pin!(stack.run(
        // The Matter stack needs (a clone of) the system event loop
        EspSystemEventLoop::take()?,
        // The Matter stack needs (a clone of) the timer service
        EspTaskTimerService::new()?,
        // The Matter stack needs (a clone of) the default ESP IDF NVS partition
        EspDefaultNvsPartition::take()?,
        // The Matter stack needs the BT/Wifi modem peripheral - and in general -
        // the Bluetooth / Wifi connections will be managed by the Matter stack itself
        // For finer-grained control, call `MatterStack::is_commissioned`,
        // `MatterStack::commission` and `MatterStack::operate`
        Peripherals::take()?.modem,
        // Hard-coded for demo purposes
        CommissioningData {
            verifier: VerifierData::new_with_pw(123456, *stack.matter().borrow()),
            discriminator: 250,
        },
        // Our `AsyncHandler` + `AsyncMetadata` impl
        (NODE, handler),
    ));

    // Just for demoing purposes:
    //
    // Run a sample loop that simulates state changes triggered by the HAL
    // Changes will be properly communicated to the Matter controllers
    // (i.e. Google Home, Alexa) and other Matter devices thanks to subscriptions
    let mut device = pin!(async {
        loop {
            // Simulate user toggling the light with a physical switch every 5 seconds
            Timer::after(Duration::from_secs(5)).await;

            // Toggle
            on_off.set(!on_off.get());

            // Let the Matter stack know that we have changed
            // the state of our Light device
            stack.notify_changed();

            info!("Light toggled");
        }
    });

    // Schedule the Matter run & the device loop together
    select(&mut matter, &mut device).coalesce().await
}

/// The Matter stack is allocated statically to avoid
/// program stack blowups.
/// It is also a mandatory requirement when the `WifiBle` stack variation is used.
static MATTER_STACK: ConstStaticCell<WifiBleMatterStack> =
    ConstStaticCell::new(WifiBleMatterStack::new(
        &BasicInfoConfig {
            vid: 0xFFF1,
            pid: 0x8000,
            hw_ver: 2,
            sw_ver: 1,
            sw_ver_str: "1",
            serial_no: "aabbccdd",
            device_name: "MyLight",
            product_name: "ACME Light",
            vendor_name: "ACME",
        },
        &dev_att::HardCodedDevAtt::new(),
        MdnsType::default(),
    ));

/// Endpoint 0 (the root endpoint) always runs
/// the hidden Matter system clusters, so we pick ID=1
const LIGHT_ENDPOINT_ID: u16 = 1;

/// The Matter Light device Node
const NODE: Node<'static> = Node {
    id: 0,
    endpoints: &[
        WifiBleMatterStack::root_metadata(),
        Endpoint {
            id: LIGHT_ENDPOINT_ID,
            device_type: DEV_TYPE_ON_OFF_LIGHT,
            clusters: &[descriptor::CLUSTER, cluster_on_off::CLUSTER],
        },
    ],
};
```

(See also [Examples](#examples))

### Advanced use cases

If the provided `MatterStack` does not cut it, users can implement their own stacks because the building blocks are also exposed as a public API.

#### Building blocks

* [Bluetooth commissioning support](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/ble.rs) with the ESP IDF Bluedroid stack (not necessary if you plan to run Matter over Ethernet)
* WiFi provisioning support via an [ESP IDF specific Matter Network Commissioning Cluster implementation](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/wifi/comm.rs)
* [Non-volatile storage for Matter persistent data (fabrics, ACLs and network connections)](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/nvs.rs) on top of the ESP IDF NVS flash API
* mDNS:
  * Optional [Matter mDNS responder implementation](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/mdns.rs) based on the ESP IDF mDNS responder (use if you need to register other services besides Matter in mDNS)
  * [UDP-multicast workarounds](https://github.com/ivmarkov/esp-idf-matter/blob/master/src/multicast.rs) for `rs-matter`'s built-in mDNS responder, addressing bugs in the Rust STD wrappers of ESP IDF

#### Future
* Device Attestation data support using secure flash storage
* Setting system time via Matter
* Matter OTA support based on the ESP IDF OTA API
* Thread networking (for ESP32H2 and ESP32C6)
* Wifi Access-Point based commissioning (for ESP32S2 which does not have Bluetooth support)

#### Additional building blocks provided by `rs-matter` directly and compatible with ESP IDF:
* UDP and (in future) TCP support
  * Enable the `async-io` and `std` features on `rs-matter` and use `async-io` sockets. The [`async-io`](https://github.com/smol-rs/async-io) crate has support for ESP IDF out of the box
* Random number generator
  * Enable the `std` feature on `rs-matter`. This way, the [`rand`](https://github.com/rust-random/rand) crate will be utilized, which has support for ESP IDF out of the box
* UNIX epoch
  * Enable the `std` feature on `rs-matter`. This way, `rs-matter` will utilize `std::time::SystemTime` which is supported by ESP IDF out of the box

## Build Prerequisites

Follow the [Prerequisites](https://github.com/esp-rs/esp-idf-template#prerequisites) section in the `esp-idf-template` crate.

## Examples

The examples could be built and flashed conveniently with [`cargo-espflash`](https://github.com/esp-rs/espflash/). To run e.g. `light` on an e.g. ESP32-C3:
(Swap the Rust target and example name with the target corresponding for your ESP32 MCU and with the example you would like to build)

with `cargo-espflash`:
```sh
$ MCU=esp32c3 cargo espflash flash --target riscv32imc-esp-espidf --example light --features examples --monitor
```

| MCU | "--target" |
| --- | ------ |
| esp32c2 | riscv32imc-esp-espidf |
| esp32c3| riscv32imc-esp-espidf |
| esp32c6| riscv32imac-esp-espidf |
| esp32h2 | riscv32imac-esp-espidf |
| esp32p4 | riscv32imafc-esp-espidf |
| esp32 | xtensa-esp32-espidf |
| esp32s2 | xtensa-esp32s2-espidf |
| esp32s3 | xtensa-esp32s3-espidf |
