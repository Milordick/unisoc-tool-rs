#[derive(Debug, Clone)]
#[allow(dead_code)] // `brand` is reference data for the device table, not used in logic
pub struct DeviceModel {
    pub name: &'static str,
    pub brand: &'static str,
    pub chip: &'static str,
    pub fdl1_addr: u32,
    pub fdl2_addr: u32,
}

pub const KNOWN_DEVICES: &[DeviceModel] = &[
    DeviceModel {
        name: "Realme C11 (RMX3231)",
        brand: "Realme",
        chip: "SC9863A_64",
        fdl1_addr: 0x00005400,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C21Y (RMX3261)",
        brand: "Realme",
        chip: "Tiger T610_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C21Y (RMX3263)",
        brand: "Realme",
        chip: "Tiger T610_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C30 (RMX3581)",
        brand: "Realme",
        chip: "Tiger T612_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C30 (RMX3623)",
        brand: "Realme",
        chip: "Tiger T612_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C31 (RMX3501)",
        brand: "Realme",
        chip: "Tiger T612_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C33 (RMX3624)",
        brand: "Realme",
        chip: "Tiger T612_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C35 (RMX3511)",
        brand: "Realme",
        chip: "Tiger T616_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme C53 (RMX3760)",
        brand: "Realme",
        chip: "Tiger T616_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Realme Narzo 50i Prime (RMX3506)",
        brand: "Realme",
        chip: "Tiger T616_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    // Generic Unisoc Chips
    DeviceModel {
        name: "Generic SC7731E",
        brand: "Unisoc",
        chip: "SC7731E",
        fdl1_addr: 0x50000000,
        fdl2_addr: 0x80000000,
    },
    DeviceModel {
        name: "Generic SC9832E",
        brand: "Unisoc",
        chip: "SC9832E",
        fdl1_addr: 0x50000000,
        fdl2_addr: 0x80000000,
    },
    DeviceModel {
        name: "Generic SC9863A (32-bit)",
        brand: "Unisoc",
        chip: "SC9863A",
        fdl1_addr: 0x50000000,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic SC9863A (64-bit)",
        brand: "Unisoc",
        chip: "SC9863A_64",
        fdl1_addr: 0x00005400,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic Tiger T606 (64-bit)",
        brand: "Unisoc",
        chip: "Tiger T606_64",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic Tiger T610 (64-bit)",
        brand: "Unisoc",
        chip: "Tiger T610_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic Tiger T612 (64-bit)",
        brand: "Unisoc",
        chip: "Tiger T612_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic Tiger T616 (64-bit)",
        brand: "Unisoc",
        chip: "Tiger T616_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Generic Tiger T700 (64-bit)",
        brand: "Unisoc",
        chip: "Tiger T700_64",
        fdl1_addr: 0x00005500,
        fdl2_addr: 0x9efffe00,
    },
    // Transsion / Infinix / Tecno Devices (from spd_dump)
    DeviceModel {
        name: "Infinix Hot 30i (X669 / X669D)",
        brand: "Infinix",
        chip: "Tiger T606_64",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Infinix Smart 6 / 7 / 8 (X6515 / X6525)",
        brand: "Infinix",
        chip: "Tiger T606 / SC9863A",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Infinix Hot 12i / 20i (X665 / X669)",
        brand: "Infinix",
        chip: "Tiger T606_64",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Tecno Spark 8C / Pop 7 (KG5k / BF6)",
        brand: "Tecno",
        chip: "Tiger T606_64",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
    DeviceModel {
        name: "Transsion Generic (Infinix/Tecno/itel)",
        brand: "Transsion",
        chip: "Tiger T606/T616",
        fdl1_addr: 0x65000800,
        fdl2_addr: 0x9efffe00,
    },
];

pub fn find_device(query: &str) -> Option<&'static DeviceModel> {
    let q = query.to_lowercase();
    KNOWN_DEVICES.iter().find(|d| {
        d.name.to_lowercase().contains(&q) || d.chip.to_lowercase().contains(&q)
    })
}
