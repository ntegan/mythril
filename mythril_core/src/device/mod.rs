use crate::error::{Error, Result};
use crate::memory::{GuestAddressSpaceViewMut, GuestPhysAddr};
use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::convert::{TryFrom, TryInto};
use core::fmt;
use core::ops::RangeInclusive;

pub mod acpi;
pub mod com;
pub mod debug;
pub mod dma;
pub mod ignore;
pub mod keyboard;
pub mod lapic;
pub mod pci;
pub mod pic;
pub mod pit;
pub mod pos;
pub mod qemu_fw_cfg;
pub mod rtc;
pub mod vga;

pub type Port = u16;

#[derive(Eq, PartialEq)]
struct PortIoRegion(RangeInclusive<Port>);

#[derive(Eq, PartialEq)]
struct MemIoRegion(RangeInclusive<GuestPhysAddr>);

impl PartialOrd for PortIoRegion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PortIoRegion {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.0.end() < other.0.start() {
            Ordering::Less
        } else if other.0.end() < self.0.start() {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

impl PartialOrd for MemIoRegion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MemIoRegion {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.0.end() < other.0.start() {
            Ordering::Less
        } else if other.0.end() < self.0.start() {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

pub enum DeviceRegion {
    PortIo(RangeInclusive<Port>),
    MemIo(RangeInclusive<GuestPhysAddr>),
}

pub trait DeviceInteraction {
    fn find_device(self, map: &DeviceMap) -> Option<&Box<dyn EmulatedDevice>>;
    fn find_device_mut(
        self,
        map: &mut DeviceMap,
    ) -> Option<&mut Box<dyn EmulatedDevice>>;
}

impl DeviceInteraction for u16 {
    fn find_device(self, map: &DeviceMap) -> Option<&Box<dyn EmulatedDevice>> {
        let range = PortIoRegion(RangeInclusive::new(self, self));
        map.portio_map.get(&range).map(|v| &**v)
    }
    fn find_device_mut(
        self,
        map: &mut DeviceMap,
    ) -> Option<&mut Box<dyn EmulatedDevice>> {
        let range = PortIoRegion(RangeInclusive::new(self, self));
        //NOTE: This is safe because all of the clones will exist in the same DeviceMap,
        //      so there cannot be other outstanding references
        map.portio_map
            .get_mut(&range)
            .map(|v| unsafe { Rc::get_mut_unchecked(v) })
    }
}

impl DeviceInteraction for GuestPhysAddr {
    fn find_device(self, map: &DeviceMap) -> Option<&Box<dyn EmulatedDevice>> {
        let range = MemIoRegion(RangeInclusive::new(self, self));
        map.memio_map.get(&range).map(|v| &**v)
    }
    fn find_device_mut(
        self,
        map: &mut DeviceMap,
    ) -> Option<&mut Box<dyn EmulatedDevice>> {
        let range = MemIoRegion(RangeInclusive::new(self, self));
        map.memio_map
            .get_mut(&range)
            .map(|v| unsafe { Rc::get_mut_unchecked(v) })
    }
}

/// A structure for looking up `EmulatedDevice`s by port or address
#[derive(Default)]
pub struct DeviceMap {
    portio_map: BTreeMap<PortIoRegion, Rc<Box<dyn EmulatedDevice>>>,
    memio_map: BTreeMap<MemIoRegion, Rc<Box<dyn EmulatedDevice>>>,
}

impl DeviceMap {
    /// Find the device that is responsible for handling an interaction
    pub fn device_for(
        &self,
        op: impl DeviceInteraction,
    ) -> Option<&Box<dyn EmulatedDevice>> {
        op.find_device(self)
    }

    pub fn device_for_mut(
        &mut self,
        op: impl DeviceInteraction,
    ) -> Option<&mut Box<dyn EmulatedDevice>> {
        op.find_device_mut(self)
    }

    pub fn register_device(
        &mut self,
        dev: Box<dyn EmulatedDevice>,
    ) -> Result<()> {
        let services = dev.services();
        let dev = Rc::new(dev);
        for region in services.into_iter() {
            match region {
                DeviceRegion::PortIo(val) => {
                    let key = PortIoRegion(val);
                    if self.portio_map.contains_key(&key) {
                        let conflict = self
                            .portio_map
                            .get_key_value(&key)
                            .expect("Could not get conflicting device")
                            .0;

                        return Err(Error::InvalidDevice(format!(
                            "I/O Port already registered: 0x{:x}-0x{:x} conflicts with existing map of 0x{:x}-0x{:x}",
                            key.0.start(), key.0.end(), conflict.0.start(), conflict.0.end()
                        )));
                    }
                    self.portio_map.insert(key, Rc::clone(&dev));
                }
                DeviceRegion::MemIo(val) => {
                    let key = MemIoRegion(val);
                    if self.memio_map.contains_key(&key) {
                        let conflict = self
                            .memio_map
                            .get_key_value(&key)
                            .expect("Could not get conflicting device")
                            .0;
                        return Err(Error::InvalidDevice(format!(
                            "Memory region already registered: 0x{:x}-0x{:x} conflicts with existing map of 0x{:x}-0x{:x}",
                            key.0.start().as_u64(), key.0.end().as_u64(), conflict.0.start().as_u64(), conflict.0.end().as_u64()
                        )));
                    }
                    self.memio_map.insert(key, Rc::clone(&dev));
                }
            }
        }
        Ok(())
    }
}

pub trait EmulatedDevice {
    fn services(&self) -> Vec<DeviceRegion>;

    fn on_mem_read(
        &mut self,
        _addr: GuestPhysAddr,
        _data: MemReadRequest,
        _space: GuestAddressSpaceViewMut,
    ) -> Result<()> {
        Err(Error::NotImplemented(
            "MemoryMapped device does not support reading".into(),
        ))
    }
    fn on_mem_write(
        &mut self,
        _addr: GuestPhysAddr,
        _data: MemWriteRequest,
        _space: GuestAddressSpaceViewMut,
    ) -> Result<()> {
        Err(Error::NotImplemented(
            "MemoryMapped device does not support writing".into(),
        ))
    }
    fn on_port_read(
        &mut self,
        _port: Port,
        _val: PortReadRequest,
        _space: GuestAddressSpaceViewMut,
    ) -> Result<()> {
        Err(Error::NotImplemented(
            "PortIo device does not support reading".into(),
        ))
    }
    fn on_port_write(
        &mut self,
        _port: Port,
        _val: PortWriteRequest,
        _space: GuestAddressSpaceViewMut,
    ) -> Result<()> {
        Err(Error::NotImplemented(
            "PortIo device does not support writing".into(),
        ))
    }
}

#[derive(Debug)]
pub enum PortReadRequest<'a> {
    OneByte(&'a mut [u8; 1]),
    TwoBytes(&'a mut [u8; 2]),
    FourBytes(&'a mut [u8; 4]),
}

#[derive(Debug)]
pub enum PortWriteRequest<'a> {
    OneByte(&'a [u8; 1]),
    TwoBytes(&'a [u8; 2]),
    FourBytes(&'a [u8; 4]),
}

impl<'a> PortReadRequest<'a> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            &Self::OneByte(ref val) => *val,
            &Self::TwoBytes(ref val) => *val,
            &Self::FourBytes(ref val) => *val,
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            &mut Self::OneByte(ref mut val) => *val,
            &mut Self::TwoBytes(ref mut val) => *val,
            &mut Self::FourBytes(ref mut val) => *val,
        }
    }

    pub fn copy_from_u32(&mut self, val: u32) {
        let arr = val.to_be_bytes();
        let len = self.len();
        self.as_mut_slice().copy_from_slice(&arr[4 - len..]);
    }
}

impl<'a> TryFrom<&'a mut [u8]> for PortReadRequest<'a> {
    type Error = Error;

    fn try_from(buff: &'a mut [u8]) -> Result<Self> {
        let res = match buff.len() {
            1 => Self::OneByte(unsafe {
                &mut *(buff.as_mut_ptr() as *mut [u8; 1])
            }),
            2 => Self::TwoBytes(unsafe {
                &mut *(buff.as_mut_ptr() as *mut [u8; 2])
            }),
            4 => Self::FourBytes(unsafe {
                &mut *(buff.as_mut_ptr() as *mut [u8; 4])
            }),
            len => {
                return Err(Error::InvalidValue(format!(
                    "Invalid slice length: {}",
                    len
                )))
            }
        };
        Ok(res)
    }
}

impl<'a> fmt::Display for PortReadRequest<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OneByte(arr) => {
                write!(f, "PortReadRequest([0x{:x}])", arr[0])
            }
            Self::TwoBytes(arr) => {
                write!(f, "PortReadRequest([0x{:x}, 0x{:x}])", arr[0], arr[1])
            }
            Self::FourBytes(arr) => write!(
                f,
                "PortReadRequest([0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}])",
                arr[0], arr[1], arr[2], arr[3]
            ),
        }
    }
}

impl<'a> PortWriteRequest<'a> {
    pub fn as_slice(&self) -> &'a [u8] {
        match *self {
            Self::OneByte(val) => val,
            Self::TwoBytes(val) => val,
            Self::FourBytes(val) => val,
        }
    }

    pub fn as_u32(&self) -> u32 {
        let arr = match self {
            Self::OneByte(val) => [0, 0, 0, val[0]],
            Self::TwoBytes(val) => [0, 0, val[0], val[1]],
            Self::FourBytes(val) => *val.clone(),
        };
        u32::from_be_bytes(arr)
    }
}

impl<'a> TryFrom<&'a [u8]> for PortWriteRequest<'a> {
    type Error = Error;

    fn try_from(buff: &'a [u8]) -> Result<Self> {
        let res = match buff.len() {
            1 => Self::OneByte(unsafe { &*(buff.as_ptr() as *const [u8; 1]) }),
            2 => Self::TwoBytes(unsafe { &*(buff.as_ptr() as *const [u8; 2]) }),
            4 => {
                Self::FourBytes(unsafe { &*(buff.as_ptr() as *const [u8; 4]) })
            }
            len => {
                return Err(Error::InvalidValue(format!(
                    "Invalid slice length: {}",
                    len
                )))
            }
        };
        Ok(res)
    }
}

impl<'a> TryInto<u8> for PortWriteRequest<'a> {
    type Error = Error;

    fn try_into(self) -> Result<u8> {
        match self {
            Self::OneByte(val) => Ok(val[0]),
            val => Err(Error::InvalidValue(format!(
                "Value {} cannot be converted to u8",
                val
            ))),
        }
    }
}

impl<'a> TryInto<u16> for PortWriteRequest<'a> {
    type Error = Error;

    fn try_into(self) -> Result<u16> {
        match self {
            Self::TwoBytes(val) => Ok(u16::from_be_bytes(*val)),
            val => Err(Error::InvalidValue(format!(
                "Value {} cannot be converted to u16",
                val
            ))),
        }
    }
}

impl<'a> TryInto<u32> for PortWriteRequest<'a> {
    type Error = Error;

    fn try_into(self) -> Result<u32> {
        match self {
            Self::FourBytes(val) => Ok(u32::from_be_bytes(*val)),
            val => Err(Error::InvalidValue(format!(
                "Value {} cannot be converted to u32",
                val
            ))),
        }
    }
}

impl<'a> fmt::Display for PortWriteRequest<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OneByte(arr) => {
                write!(f, "PortWriteRequest([0x{:x}])", arr[0])
            }
            Self::TwoBytes(arr) => {
                write!(f, "PortWriteRequest([0x{:x}, 0x{:x}])", arr[0], arr[1])
            }
            Self::FourBytes(arr) => write!(
                f,
                "PortWriteRequest([0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}])",
                arr[0], arr[1], arr[2], arr[3]
            ),
        }
    }
}

pub struct MemWriteRequest<'a> {
    data: &'a [u8],
}

impl fmt::Debug for MemWriteRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemWriteRequest")
            .field("data", &format_args!("{:02x?}", self.data))
            .finish()
    }
}

impl<'a> MemWriteRequest<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn as_slice(&self) -> &'a [u8] {
        self.data
    }
}

impl<'a> fmt::Display for MemWriteRequest<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemWriteRequest({:?})", self.data)
    }
}

impl<'a> TryInto<u8> for MemWriteRequest<'a> {
    type Error = Error;

    fn try_into(self) -> Result<u8> {
        if self.data.len() == 1 {
            Ok(self.data[0])
        } else {
            Err(Error::InvalidValue(format!(
                "Value {} cannot be converted to u8",
                self
            )))
        }
    }
}

#[derive(Debug)]
pub struct MemReadRequest<'a> {
    data: &'a mut [u8],
}

impl<'a> MemReadRequest<'a> {
    pub fn new(data: &'a mut [u8]) -> Self {
        Self { data }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.data
    }
}

impl<'a> fmt::Display for MemReadRequest<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemReadRequest({:?})", self.data)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::device::com::*;
    use crate::memory::{
        GuestAddressSpace, GuestAddressSpaceViewMut, GuestPhysAddr,
    };
    use core::convert::TryInto;

    fn define_test_view() -> GuestAddressSpaceViewMut<'static> {
        let space: &'static mut GuestAddressSpace =
            Box::leak(Box::new(GuestAddressSpace::new().unwrap()));
        GuestAddressSpaceViewMut::new(GuestPhysAddr::new(0), space)
    }

    // This is just a dummy device so we can have arbitrary port ranges
    // for testing.
    struct DummyDevice {
        services: Vec<RangeInclusive<Port>>,
    }

    impl DummyDevice {
        fn new(services: Vec<RangeInclusive<Port>>) -> Box<dyn EmulatedDevice> {
            Box::new(Self { services })
        }
    }

    impl EmulatedDevice for DummyDevice {
        fn services(&self) -> Vec<DeviceRegion> {
            self.services
                .iter()
                .map(|x| DeviceRegion::PortIo(x.clone()))
                .collect()
        }
    }

    #[test]
    fn test_memmap_write_to_portio_fails() {
        let view = define_test_view();
        let mut com = ComDevice::new(0, 0);
        let addr = GuestPhysAddr::new(0);
        let data = [0u8; 4];
        let req = MemWriteRequest::new(&data);
        assert_eq!(com.on_mem_write(addr, req, view).is_err(), true);
    }

    #[test]
    fn test_device_map() {
        let mut map = DeviceMap::default();
        let com = ComDevice::new(0, 0);
        map.register_device(com).unwrap();
        let _dev = map.device_for(0u16).unwrap();

        assert_eq!(map.device_for(10u16).is_none(), true);
    }

    #[test]
    fn test_write_request_try_from() {
        let val: Result<PortWriteRequest> =
            [0x12, 0x34, 0x56, 0x78][..].try_into();
        assert_eq!(val.is_ok(), true);

        let val: Result<PortWriteRequest> = [0x12, 0x34, 0x56][..].try_into();
        assert_eq!(val.is_err(), true);

        let val: PortWriteRequest =
            [0x12, 0x34, 0x56, 0x78][..].try_into().unwrap();
        assert_eq!(val.as_u32(), 0x12345678);

        let val: PortWriteRequest = [0x12, 0x34][..].try_into().unwrap();
        assert_eq!(val.as_u32(), 0x1234);
    }

    #[test]
    fn test_portio_value_read() {
        let mut arr = [0x00, 0x00];
        let mut val = PortReadRequest::TwoBytes(&mut arr);
        val.copy_from_u32(0x1234u32);
        assert_eq!([0x12, 0x34], val.as_slice());
        assert_eq!(0x1234, u16::from_be_bytes(arr));
    }

    #[test]
    fn test_conflicting_portio_device() {
        let mut map = DeviceMap::default();
        let com = ComDevice::new(0, 0);
        map.register_device(com).unwrap();
        let com = ComDevice::new(0, 0);

        assert!(map.register_device(com).is_err());
    }

    #[test]
    fn test_fully_overlapping_portio_device() {
        // region 2 fully inside region 1
        let services = vec![0..=10, 2..=8];
        let dummy = DummyDevice::new(services);
        let mut map = DeviceMap::default();

        assert!(map.register_device(dummy).is_err());
    }

    #[test]
    fn test_fully_encompassing_portio_device() {
        // region 1 fully inside region 2
        let services = vec![2..=8, 0..=10];
        let dummy = DummyDevice::new(services);
        let mut map = DeviceMap::default();

        assert!(map.register_device(dummy).is_err());
    }

    #[test]
    fn test_partially_overlapping_tail_portio_device() {
        // region 1 and region 2 partially overlap at the tail of region 1 and
        // the start of region 2
        let services = vec![0..=4, 3..=8];
        let dummy = DummyDevice::new(services);
        let mut map = DeviceMap::default();

        assert!(map.register_device(dummy).is_err());
    }

    #[test]
    fn test_partially_overlapping_head_portio_device() {
        // region 1 and region 2 partially overlap at the start of region 1 and
        // the tail of region 2
        let services = vec![3..=8, 0..=4];
        let dummy = DummyDevice::new(services);
        let mut map = DeviceMap::default();

        assert!(map.register_device(dummy).is_err());
    }

    #[test]
    fn test_non_overlapping_portio_device() {
        // region 1 and region 2 don't overlap
        let services = vec![0..=3, 4..=8];
        let dummy = DummyDevice::new(services);
        let mut map = DeviceMap::default();

        assert!(map.register_device(dummy).is_ok());
    }
}
