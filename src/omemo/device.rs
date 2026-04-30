use tokio_xmpp::minidom::Element;

use super::{NS_OMEMO_V0, nc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: u32,
    pub label: Option<String>,
    pub labelsig: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceList {
    pub devices: Vec<Device>,
}

pub fn parse_device_list(element: &Element) -> Option<DeviceList> {
    if element.name() != "list" || element.ns() != NS_OMEMO_V0 {
        return None;
    }

    let mut devices = Vec::new();
    for dev_el in element.children() {
        if dev_el.name() != "device" || dev_el.ns() != NS_OMEMO_V0 {
            continue;
        }
        let id: u32 = dev_el.attr("id")?.parse().ok()?;
        let label = dev_el.attr("label").map(|s| s.to_string());
        let labelsig = dev_el.attr("labelsig").map(|s| s.to_string());
        devices.push(Device {
            id,
            label,
            labelsig,
        });
    }

    Some(DeviceList { devices })
}

pub fn build_device_list_element_v0(devices: &[Device]) -> Element {
    let mut el = Element::builder("list", NS_OMEMO_V0).build();
    for dev in devices {
        let mut builder =
            Element::builder("device", NS_OMEMO_V0).attr(nc("id"), dev.id.to_string());
        if let Some(ref label) = dev.label {
            builder = builder.attr(nc("label"), label.clone());
        }
        if let Some(ref labelsig) = dev.labelsig {
            builder = builder.attr(nc("labelsig"), labelsig.clone());
        }
        el.append_child(builder.build());
    }
    el
}
