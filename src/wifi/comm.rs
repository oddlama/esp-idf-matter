use embassy_sync::blocking_mutex::raw::RawMutex;

use log::{error, info};

use rs_matter::data_model::objects::{
    AsyncHandler, AttrDataEncoder, AttrDataWriter, AttrDetails, AttrType, CmdDataEncoder,
    CmdDetails, Dataver,
};
use rs_matter::data_model::sdm::nw_commissioning::{
    AddWifiNetworkRequest, Attributes, Commands, ConnectNetworkRequest, NetworkCommissioningStatus,
    NetworkConfigResponse, NwInfo, RemoveNetworkRequest, ReorderNetworkRequest, ResponseCommands,
    ScanNetworksRequest, WIFI_CLUSTER,
};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::interaction_model::core::IMStatusCode;
use rs_matter::interaction_model::messages::ib::Status;
use rs_matter::tlv::{FromTLV, OctetStr, TLVElement, TagType, ToTLV};
use rs_matter::transport::exchange::Exchange;
use rs_matter::utils::rand::Rand;

use super::{WifiContext, WifiCredentials};

pub struct WifiCommCluster<'a, const N: usize, M>
where
    M: RawMutex,
{
    data_ver: Dataver,
    networks: &'a WifiContext<N, M>,
}

impl<'a, const N: usize, M> WifiCommCluster<'a, N, M>
where
    M: RawMutex,
{
    pub fn new(rand: Rand, networks: &'a WifiContext<N, M>) -> Self {
        Self {
            data_ver: Dataver::new(rand),
            networks,
        }
    }

    async fn read(
        &self,
        attr: &AttrDetails<'_>,
        encoder: AttrDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        if let Some(mut writer) = encoder.with_dataver(self.data_ver.get())? {
            if attr.is_system() {
                WIFI_CLUSTER.read(attr.attr_id, writer)
            } else {
                match attr.attr_id.try_into()? {
                    Attributes::MaxNetworks => AttrType::<u8>::new().encode(writer, N as u8),
                    Attributes::Networks => {
                        writer.start_array(AttrDataWriter::TAG)?;

                        self.networks.state.lock(|state| {
                            let state = state.borrow();

                            for network in &state.networks {
                                let nw_info = NwInfo {
                                    network_id: OctetStr(network.ssid.as_str().as_bytes()),
                                    connected: state
                                        .status
                                        .as_ref()
                                        .map(|status| {
                                            *status.ssid == network.ssid
                                                && matches!(
                                                    status.status,
                                                    NetworkCommissioningStatus::Success
                                                )
                                        })
                                        .unwrap_or(false),
                                };

                                nw_info.to_tlv(&mut writer, TagType::Anonymous)?;
                            }

                            Ok::<_, Error>(())
                        })?;

                        writer.end_container()?;
                        writer.complete()
                    }
                    Attributes::ScanMaxTimeSecs => AttrType::new().encode(writer, 30_u8),
                    Attributes::ConnectMaxTimeSecs => AttrType::new().encode(writer, 60_u8),
                    Attributes::InterfaceEnabled => AttrType::new().encode(writer, true),
                    Attributes::LastNetworkingStatus => self.networks.state.lock(|state| {
                        AttrType::new().encode(
                            writer,
                            state.borrow().status.as_ref().map(|o| o.status as u8),
                        )
                    }),
                    Attributes::LastNetworkID => self.networks.state.lock(|state| {
                        AttrType::new().encode(
                            writer,
                            state
                                .borrow()
                                .status
                                .as_ref()
                                .map(|o| OctetStr(o.ssid.as_str().as_bytes())),
                        )
                    }),
                    Attributes::LastConnectErrorValue => self.networks.state.lock(|state| {
                        AttrType::new()
                            .encode(writer, state.borrow().status.as_ref().map(|o| o.value))
                    }),
                }
            }
        } else {
            Ok(())
        }
    }

    async fn invoke(
        &self,
        exchange: &Exchange<'_>,
        cmd: &CmdDetails<'_>,
        data: &TLVElement<'_>,
        encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        match cmd.cmd_id.try_into()? {
            Commands::ScanNetworks => {
                info!("ScanNetworks");
                self.scan_networks(exchange, &ScanNetworksRequest::from_tlv(data)?, encoder)
                    .await?;
            }
            Commands::AddOrUpdateWifiNetwork => {
                info!("AddOrUpdateWifiNetwork");
                self.add_network(exchange, &AddWifiNetworkRequest::from_tlv(data)?, encoder)
                    .await?;
            }
            Commands::RemoveNetwork => {
                info!("RemoveNetwork");
                self.remove_network(exchange, &RemoveNetworkRequest::from_tlv(data)?, encoder)
                    .await?;
            }
            Commands::ConnectNetwork => {
                info!("ConnectNetwork");
                self.connect_network(exchange, &ConnectNetworkRequest::from_tlv(data)?, encoder)
                    .await?;
            }
            Commands::ReorderNetwork => {
                info!("ReorderNetwork");
                self.reorder_network(exchange, &ReorderNetworkRequest::from_tlv(data)?, encoder)
                    .await?;
            }
            other => {
                error!("{other:?} (not supported)");
                Err(ErrorCode::CommandNotFound)?
            }
        }

        self.data_ver.changed();

        Ok(())
    }

    async fn scan_networks(
        &self,
        _exchange: &Exchange<'_>,
        _req: &ScanNetworksRequest<'_>,
        encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        let mut tw = encoder.with_command(ResponseCommands::ScanNetworksResponse as _)?;

        Status::new(IMStatusCode::Busy, 0).to_tlv(&mut tw, TagType::Anonymous)?;

        Ok(())
    }

    async fn add_network(
        &self,
        exchange: &Exchange<'_>,
        req: &AddWifiNetworkRequest<'_>,
        encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        // TODO: Check failsafe status

        self.networks.state.lock(|state| {
            let mut state = state.borrow_mut();

            let index = state
                .networks
                .iter()
                .position(|conf| conf.ssid.as_str().as_bytes() == req.ssid.0);

            let mut tw = encoder.with_command(ResponseCommands::NetworkConfigResponse as _)?;

            if let Some(index) = index {
                // Update
                state.networks[index].ssid = core::str::from_utf8(req.ssid.0)
                    .unwrap()
                    .try_into()
                    .unwrap();
                state.networks[index].password = core::str::from_utf8(req.credentials.0)
                    .unwrap()
                    .try_into()
                    .unwrap();

                state.changed = true;
                exchange.matter().notify_changed();

                NetworkConfigResponse {
                    status: NetworkCommissioningStatus::Success,
                    debug_text: None,
                    network_index: Some(index as _),
                }
                .to_tlv(&mut tw, TagType::Anonymous)?;
            } else {
                // Add
                let network = WifiCredentials {
                    // TODO
                    ssid: core::str::from_utf8(req.ssid.0)
                        .unwrap()
                        .try_into()
                        .unwrap(),
                    password: core::str::from_utf8(req.credentials.0)
                        .unwrap()
                        .try_into()
                        .unwrap(),
                };

                if state.networks.push(network).is_ok() {
                    state.changed = true;
                    exchange.matter().notify_changed();

                    NetworkConfigResponse {
                        status: NetworkCommissioningStatus::Success,
                        debug_text: None,
                        network_index: Some(state.networks.len() as _),
                    }
                    .to_tlv(&mut tw, TagType::Anonymous)?;
                } else {
                    NetworkConfigResponse {
                        status: NetworkCommissioningStatus::BoundsExceeded,
                        debug_text: None,
                        network_index: None,
                    }
                    .to_tlv(&mut tw, TagType::Anonymous)?;
                }
            }

            Ok(())
        })
    }

    async fn remove_network(
        &self,
        exchange: &Exchange<'_>,
        req: &RemoveNetworkRequest<'_>,
        encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        // TODO: Check failsafe status

        self.networks.state.lock(|state| {
            let mut state = state.borrow_mut();

            let index = state
                .networks
                .iter()
                .position(|conf| conf.ssid.as_str().as_bytes() == req.network_id.0);

            let mut tw = encoder.with_command(ResponseCommands::NetworkConfigResponse as _)?;

            if let Some(index) = index {
                // Found
                state.networks.remove(index);
                state.changed = true;
                exchange.matter().notify_changed();

                NetworkConfigResponse {
                    status: NetworkCommissioningStatus::Success,
                    debug_text: None,
                    network_index: Some(index as _),
                }
                .to_tlv(&mut tw, TagType::Anonymous)?;
            } else {
                // Not found
                NetworkConfigResponse {
                    status: NetworkCommissioningStatus::NetworkIdNotFound,
                    debug_text: None,
                    network_index: None,
                }
                .to_tlv(&mut tw, TagType::Anonymous)?;
            }

            Ok(())
        })
    }

    async fn connect_network(
        &self,
        _exchange: &Exchange<'_>,
        req: &ConnectNetworkRequest<'_>,
        _encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        // TODO: Check failsafe status

        // Non-concurrent commissioning scenario (i.e. only BLE is active, and the ESP IDF co-exist mode is not enabled)
        // Notify that we have received a connect command

        self.networks.state.lock(|state| {
            let mut state = state.borrow_mut();

            state.connect_requested = Some(
                core::str::from_utf8(req.network_id.0)
                    .unwrap()
                    .try_into()
                    .unwrap(),
            );
        });

        self.networks.network_connect_requested.notify();

        // Block forever waitinng for the firware to restart
        core::future::pending().await
    }

    async fn reorder_network(
        &self,
        exchange: &Exchange<'_>,
        req: &ReorderNetworkRequest<'_>,
        encoder: CmdDataEncoder<'_, '_, '_>,
    ) -> Result<(), Error> {
        // TODO: Check failsafe status

        self.networks.state.lock(|state| {
            let mut state = state.borrow_mut();

            let index = state
                .networks
                .iter()
                .position(|conf| conf.ssid.as_str().as_bytes() == req.network_id.0);

            let mut tw = encoder.with_command(ResponseCommands::NetworkConfigResponse as _)?;

            if let Some(index) = index {
                // Found

                if req.index < state.networks.len() as u8 {
                    let conf = state.networks.remove(index);
                    state
                        .networks
                        .insert(req.index as usize, conf)
                        .map_err(|_| ())
                        .unwrap();

                    state.changed = true;
                    exchange.matter().notify_changed();

                    NetworkConfigResponse {
                        status: NetworkCommissioningStatus::Success,
                        debug_text: None,
                        network_index: Some(req.index as _),
                    }
                    .to_tlv(&mut tw, TagType::Anonymous)?;
                } else {
                    NetworkConfigResponse {
                        status: NetworkCommissioningStatus::OutOfRange,
                        debug_text: None,
                        network_index: Some(req.index as _),
                    }
                    .to_tlv(&mut tw, TagType::Anonymous)?;
                }
            } else {
                // Not found
                NetworkConfigResponse {
                    status: NetworkCommissioningStatus::NetworkIdNotFound,
                    debug_text: None,
                    network_index: None,
                }
                .to_tlv(&mut tw, TagType::Anonymous)?;
            }

            Ok(())
        })
    }
}

impl<'a, const N: usize, M> AsyncHandler for WifiCommCluster<'a, N, M>
where
    M: RawMutex,
{
    async fn read<'m>(
        &'m self,
        attr: &'m AttrDetails<'_>,
        encoder: AttrDataEncoder<'m, '_, '_>,
    ) -> Result<(), Error> {
        WifiCommCluster::read(self, attr, encoder).await
    }

    async fn invoke<'m>(
        &'m self,
        exchange: &'m Exchange<'_>,
        cmd: &'m CmdDetails<'_>,
        data: &'m TLVElement<'_>,
        encoder: CmdDataEncoder<'m, '_, '_>,
    ) -> Result<(), Error> {
        WifiCommCluster::invoke(self, exchange, cmd, data, encoder).await
    }
}

// impl ChangeNotifier<()> for WifiCommCluster {
//     fn consume_change(&mut self) -> Option<()> {
//         self.data_ver.consume_change(())
//     }
// }