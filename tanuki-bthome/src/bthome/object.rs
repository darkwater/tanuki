use bytes::Buf;
use tanuki_common::capabilities::sensor::SensorValue;

#[derive(Debug, PartialEq)]
pub enum Object {
    Battery(f32),
    Temperature(f32),
    Humidity(f32),
    Voltage(f32),
    Power(bool),
    Rssi(i16),
}

impl Object {
    pub fn topic(&self) -> &'static str {
        match self {
            Object::Battery(_) => "battery",
            Object::Temperature(_) => "temperature",
            Object::Humidity(_) => "humidity",
            Object::Voltage(_) => "voltage",
            Object::Power(_) => "power",
            Object::Rssi(_) => "rssi",
        }
    }

    pub fn unit(&self) -> &'static str {
        match self {
            Object::Battery(_) => "%",
            Object::Temperature(_) => "°C",
            Object::Humidity(_) => "%",
            Object::Voltage(_) => "V",
            Object::Power(_) => "",
            Object::Rssi(_) => "dBm",
        }
    }

    pub fn value(&self) -> SensorValue {
        match self {
            Object::Battery(v) => SensorValue::Number(*v),
            Object::Temperature(v) => SensorValue::Number(*v),
            Object::Humidity(v) => SensorValue::Number(*v),
            Object::Voltage(v) => SensorValue::Number(*v),
            Object::Power(v) => SensorValue::Boolean(*v),
            Object::Rssi(v) => SensorValue::Number(*v as f32),
        }
    }
}

impl Object {
    pub fn decode(mut data: impl Buf) -> Vec<Object> {
        data.copy_to_bytes(3);

        let mut out = vec![];

        while data.has_remaining() {
            let header = data.get_u8();
            let len = header & 0b11111;
            let ty = header >> 5;
            tracing::trace!("len: {}, ty: {}", len, ty);

            let mut data = data.copy_to_bytes(len as usize);
            tracing::trace!("{:#02x?}", &data[..]);

            let object_id = data.get_u8();
            let value = match (len, ty) {
                (2, 0) => data.get_u8() as f32,
                (3, 0) => data.get_u16_le() as f32,
                (2, 1) => data.get_i8() as f32,
                (3, 1) => data.get_i16_le() as f32,
                (5, 2) => data.get_f32_le(),
                _ => {
                    tracing::warn!("unimplemented length/type combo: len={}, type={}", len, ty);
                    continue;
                }
            };

            let obj = match object_id {
                0x01 => Object::Battery(value),
                0x02 => Object::Temperature(value * 0.01),
                0x03 => Object::Humidity(value * 0.01),
                0x0c => Object::Voltage(value * 0.001),
                0x10 => Object::Power(value > 0.),
                _ => {
                    tracing::warn!("unknown object id: {:#02x}", object_id);
                    continue;
                }
            };

            out.push(obj);
        }

        out
    }
}
