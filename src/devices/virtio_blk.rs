//! The virtio_blk module implements a virtio block device.
//!
//! The spec for Virtual I/O Device (VIRTIO) Version 1.1:
//! https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html
//! 5.2 Block Device:
//! https://docs.oasis-open.org/virtio/virtio/v1.1/cs01/virtio-v1.1-cs01.html#x1-2390002

use crate::bus::VIRTIO_BASE;
use crate::cpu::{Cpu, BYTE, DOUBLEWORD, HALFWORD, WORD};
use crate::exception::Exception;

/// The interrupt request of virtio.
pub const VIRTIO_IRQ: u64 = 1;

/// The size of `VRingDesc` struct.
const VRING_DESC_SIZE: u64 = 16;
/// The number of virtio descriptors. It must be a power of two.
const QUEUE_SIZE: u64 = 8;
/// The size of a sector.
const SECTOR_SIZE: u64 = 512;

// 4.2.2 MMIO Device Register Layout
// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1460002
/// Magic value. Always return 0x74726976 (a Little Endian equivalent of the “virt” string).
const VIRTIO_MAGIC: u64 = VIRTIO_BASE + 0x000;
/// Device version number. 1 is legacy.
const VIRTIO_VERSION: u64 = VIRTIO_BASE + 0x004;
/// Virtio Subsystem Device ID. 1 is network, 2 is block device.
const VIRTIO_DEVICE_ID: u64 = VIRTIO_BASE + 0x008;
/// Virtio Subsystem Vendor ID. Always return 0x554d4551
const VIRTIO_VENDOR_ID: u64 = VIRTIO_BASE + 0x00c;
/// Flags representing features the device supports. Access to this register returns bits
/// DeviceFeaturesSel ∗ 32 to (DeviceFeaturesSel ∗ 32) + 31.
const VIRTIO_DEVICE_FEATURES: u64 = VIRTIO_BASE + 0x010;
/// Device (host) features word selection.
const VIRTIO_DEVICE_FEATURES_SEL: u64 = VIRTIO_BASE + 0x014;
/// Flags representing device features understood and activated by the driver. Access to this
/// register sets bits DriverFeaturesSel ∗ 32 to (DriverFeaturesSel ∗ 32) + 31.
const VIRTIO_DRIVER_FEATURES: u64 = VIRTIO_BASE + 0x020;
/// Activated (guest) features word selection.
const VIRTIO_DRIVER_FEATURES_SEL: u64 = VIRTIO_BASE + 0x024;
// 4.2.4 Legacy interface
// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1560004
/// Guest page size. The driver writes the guest page size in bytes to the register during
/// initialization, before any queues are used. This value should be a power of 2 and is used by
/// the device to calculate the Guest address of the first queue page. Write-only.
const VIRTIO_GUEST_PAGE_SIZE: u64 = VIRTIO_BASE + 0x028;
/// Virtual queue index. Writing to this register selects the virtual queue that the following
/// operations on the QueueNumMax, QueueNum, QueueAlign and QueuePFN registers apply to. The index
/// number of the first queue is zero (0x0). Write-only.
const VIRTIO_QUEUE_SEL: u64 = VIRTIO_BASE + 0x030;
/// Maximum virtual queue size. Reading from the register returns the maximum size of the queue the
/// device is ready to process or zero (0x0) if the queue is not available. This applies to the
/// queue selected by writing to QueueSel and is allowed only when QueuePFN is set to zero (0x0),
/// so when the queue is not actively used. Read-only. In QEMU, `VIRTIO_COUNT = 8`.
const VIRTIO_QUEUE_NUM_MAX: u64 = VIRTIO_BASE + 0x034;
/// Virtual queue size. Queue size is the number of elements in the queue, therefore size of the
/// descriptor table and both available and used rings. Writing to this register notifies the
/// device what size of the queue the driver will use. This applies to the queue selected by
/// writing to QueueSel. Write-only.
const VIRTIO_QUEUE_NUM: u64 = VIRTIO_BASE + 0x038;
/// Used Ring alignment in the virtual queue.
const VIRTIO_QUEUE_ALIGN: u64 = VIRTIO_BASE + 0x03c;
/// Guest physical page number of the virtual queue. Writing to this register notifies the device
/// about location of the virtual queue in the Guest’s physical address space. This value is the
/// index number of a page starting with the queue Descriptor Table. Value zero (0x0) means
/// physical address zero (0x00000000) and is illegal. When the driver stops using the queue it
/// writes zero (0x0) to this register. Reading from this register returns the currently used page
/// number of the queue, therefore a value other than zero (0x0) means that the queue is in use.
/// Both read and write accesses apply to the queue selected by writing to QueueSel.
const VIRTIO_QUEUE_PFN: u64 = VIRTIO_BASE + 0x040;
// 4.2.2 MMIO Device Register Layout
// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1460002
/// Queue notifier. Writing a queue index to this register notifies the device that there are new
/// buffers to process in the queue. Write-only.
const VIRTIO_QUEUE_NOTIFY: u64 = VIRTIO_BASE + 0x050;
/// Interrupt status. Reading from this register returns a bit mask of events that caused the
/// device interrupt to be asserted.
const VIRTIO_MMIO_INTERRUPT_STATUS: u64 = VIRTIO_BASE + 0x060;
/// Interrupt acknowledge. Writing a value with bits set as defined in InterruptStatus to this
/// register notifies the device that events causing the interrupt have been handled.
const VIRTIO_MMIO_INTERRUPT_ACK: u64 = VIRTIO_BASE + 0x064;
/// Device status. Reading from this register returns the current device status flags. Writing
/// non-zero values to this register sets the status flags, indicating the driver progress. Writing
/// zero (0x0) to this register triggers a device reset.
const VIRTIO_STATUS: u64 = VIRTIO_BASE + 0x070;
/// Configuration space.
const VIRTIO_CONFIG: u64 = VIRTIO_BASE + 0x100;
const VIRTIO_CONFIG_END: u64 = VIRTIO_CONFIG + 0x8;

/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-250001
///
/// ```c
/// struct virtq {
///   struct virtq_desc desc[ Queue Size ];
///   struct virtq_avail avail;
///   u8 pad[ Padding ]; // Padding to the next Queue Align boundary.
///   struct virtq_used used;
/// };
/// ```
struct _Virtq {
    /// The actual descriptors (16 bytes each)
    /// The number of descriptors in the table is defined by the queue size for this virtqueue.
    desc: Vec<VirtqDesc>,
    /// A ring of available descriptor heads with free-running index.
    avail: _VirtqAvail,
    /// A ring of used descriptor heads with free-running index.
    used: _VirtqUsed,
}

/// "The descriptor table refers to the buffers the driver is using for the device. addr is a
/// physical address, and the buffers can be chained via next. Each descriptor describes a buffer
/// which is read-only for the device (“device-readable”) or write-only for the device
/// (“device-writable”), but a chain of descriptors can contain both device-readable and
/// device-writable buffers."
///
/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-320005
///
/// ```c
/// /* This marks a buffer as continuing via the next field. */
/// #define VIRTQ_DESC_F_NEXT 1
/// /* This marks a buffer as device write-only (otherwise device read-only). */
/// #define VIRTQ_DESC_F_WRITE 2
/// /* This means the buffer contains a list of buffer descriptors. */
/// #define VIRTQ_DESC_F_INDIRECT 4
///
/// struct virtq_desc {
///   le64 addr;
///   le32 len;
///   le16 flags;
///   le16 next;
/// };
/// ```
struct VirtqDesc {
    /// Address (guest-physical).
    addr: u64,
    /// Length.
    len: u64,
    /// The flags as indicated VIRTQ_DESC_F_NEXT/VIRTQ_DESC_F_WRITE/VIRTQ_DESC_F_INDIRECT.
    flags: u64,
    /// Next field if flags & NEXT.
    next: u64,
}

impl VirtqDesc {
    /// Create a new virtqueue descriptor based on the address that stores the content of the
    /// descriptor.
    fn new(cpu: &mut Cpu, addr: u64) -> Result<Self, Exception> {
        Ok(Self {
            addr: cpu.bus.read(addr, DOUBLEWORD)?,
            len: cpu.bus.read(addr.wrapping_add(8), WORD)?,
            flags: cpu.bus.read(addr.wrapping_add(12), HALFWORD)?,
            next: cpu.bus.read(addr.wrapping_add(14), HALFWORD)?,
        })
    }
}

/// "The driver uses the available ring to offer buffers to the device: each ring entry refers to
/// the head of a descriptor chain. It is only written by the driver and read by the device."
///
/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-380006
///
/// ```c
/// #define VIRTQ_AVAIL_F_NO_INTERRUPT 1
/// struct virtq_avail {
///   le16 flags;
///   le16 idx;
///   le16 ring[ /* Queue Size */ ];
///   le16 used_event; /* Only if VIRTIO_F_EVENT_IDX */
/// };
/// ```
struct _VirtqAvail {
    flags: u16,
    /// Indicates where the driver would put the next descriptor entry in the ring (modulo the
    /// queue size). Starts at 0 and increases.
    idx: u16,
    ring: Vec<u16>,
    used_event: u16,
}

/// "The used ring is where the device returns buffers once it is done with them: it is only
/// written to by the device, and read by the driver."
///
/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-430008
///
/// ```c
/// #define VIRTQ_USED_F_NO_NOTIFY 1
/// struct virtq_used {
///   le16 flags;
///   le16 idx;
///   struct virtq_used_elem ring[ /* Queue Size */];
///   le16 avail_event; /* Only if VIRTIO_F_EVENT_IDX */
/// };
/// ```
struct _VirtqUsed {
    flags: u16,
    /// Indicates where the device would put the next descriptor entry in the ring (modulo the
    /// queue size). Starts at 0 and increases.
    idx: u16,
    ring: Vec<_VirtqUsedElem>,
    avail_event: u16,
}

/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-430008
///
/// ```c
/// struct virtq_used_elem {
///   le32 id;
///   le32 len;
/// };
/// ```
struct _VirtqUsedElem {
    /// Index of start of used descriptor chain. Indicates the head entry of the descriptor chain
    /// describing the buffer (this matches an entry placed in the available ring by the guest
    /// earlier).
    id: u32,
    /// Total length of the descriptor chain which was used (written to).
    len: u32,
}

/// Paravirtualized drivers for IO virtualization.
pub struct Virtio {
    id: u64,
    device_features: [u32; 2],
    device_features_sel: u32,
    driver_features: [u32; 2],
    driver_features_sel: u32,
    guest_page_size: u32,
    queue_sel: u32,
    queue_num: u32,
    queue_align: u32,
    queue_pfn: u32,
    queue_notify: u32,
    interrupt_status: u32,
    /// "The device status field provides a simple low-level indication of the completed steps of
    /// this sequence.
    /// The device MUST initialize device status to 0 upon reset."
    /// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-100001
    status: u32,
    config: [u8; 8],
    disk: Vec<u8>,
}

impl Virtio {
    /// Create a new virtio object.
    pub fn new() -> Self {
        Self {
            id: 0,
            device_features: [0; 2],
            device_features_sel: 0,
            driver_features: [0; 2],
            driver_features_sel: 0,
            guest_page_size: 0,
            queue_sel: 0,
            queue_num: 0,
            queue_align: 0,
            queue_pfn: 0,
            queue_notify: 9999, // TODO: what is the correct initial value?
            interrupt_status: 0,
            // "The device MUST initialize device status to 0 upon reset."
            // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-120002
            status: 0,
            config: [0; 8],
            disk: Vec::new(),
        }
    }

    /// Return true if an interrupt is pending.
    pub fn is_interrupting(&mut self) -> bool {
        if self.queue_notify != 9999 {
            self.queue_notify = 9999;
            return true;
        }
        false
    }

    /// Set the binary in the virtio disk.
    pub fn initialize(&mut self, binary: Vec<u8>) {
        self.disk.extend(binary.iter().cloned());
    }

    /// Load `size`-bit data from a register located at `addr` in the virtio block device.
    pub fn read(&self, addr: u64, size: u8) -> Result<u64, Exception> {
        if size == DOUBLEWORD {
            return Err(Exception::LoadAccessFault);
        }

        let value = match addr {
            VIRTIO_MAGIC => 0x74726976, // A Little Endian equivalent of the “virt” string.
            VIRTIO_VERSION => 0x1,      // Legacy devices (see 4.2.4 Legacy interface) used 0x1.
            VIRTIO_DEVICE_ID => 0x2,    // Block device.
            // See https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/virtio_disk.c#L86
            VIRTIO_VENDOR_ID => 0x554d4551,
            VIRTIO_DEVICE_FEATURES => self.device_features[self.device_features_sel as usize],
            VIRTIO_QUEUE_NUM_MAX => 8,
            VIRTIO_QUEUE_PFN => self.queue_pfn,
            VIRTIO_MMIO_INTERRUPT_STATUS => self.interrupt_status,
            VIRTIO_STATUS => self.status,
            VIRTIO_CONFIG..=VIRTIO_CONFIG_END => {
                if size != BYTE {
                    return Err(Exception::LoadAccessFault);
                }
                let index = addr - VIRTIO_CONFIG;
                self.config[index as usize] as u32
            }
            _ => return Err(Exception::LoadAccessFault),
        };
        Ok(value as u64)
    }

    /// Store `size`-bit data to a register located at `addr` in the virtio block device.
    pub fn write(&mut self, addr: u64, value: u64, size: u8) -> Result<(), Exception> {
        if size == DOUBLEWORD {
            return Err(Exception::StoreAMOAccessFault);
        }

        match addr {
            VIRTIO_DEVICE_FEATURES_SEL => self.device_features_sel = value as u32,
            VIRTIO_DRIVER_FEATURES => {
                self.driver_features[self.driver_features_sel as usize] = value as u32
            }
            VIRTIO_DRIVER_FEATURES_SEL => self.driver_features_sel = value as u32,
            VIRTIO_GUEST_PAGE_SIZE => self.guest_page_size = value as u32,
            VIRTIO_QUEUE_SEL => self.queue_sel = value as u32,
            VIRTIO_QUEUE_NUM => self.queue_num = value as u32,
            VIRTIO_QUEUE_ALIGN => self.queue_align = value as u32,
            VIRTIO_QUEUE_PFN => self.queue_pfn = value as u32,
            VIRTIO_QUEUE_NOTIFY => self.queue_notify = value as u32,
            VIRTIO_MMIO_INTERRUPT_ACK => {
                if (value & 0x1) == 1 {
                    self.interrupt_status &= !0x1;
                } else {
                    panic!(
                        "unexpected value for VIRTIO_MMIO_INTERRUPT_ACK: {:#x}",
                        value
                    );
                }
            }
            VIRTIO_STATUS => self.status = value as u32,
            VIRTIO_CONFIG..=VIRTIO_CONFIG_END => {
                if size != BYTE {
                    return Err(Exception::StoreAMOAccessFault);
                }
                let index = addr - VIRTIO_CONFIG;
                self.config[index as usize] = (value >> (index * 8)) as u8;
            }
            _ => return Err(Exception::StoreAMOAccessFault),
        }
        Ok(())
    }

    fn get_new_id(&mut self) -> u64 {
        self.id = self.id.wrapping_add(1);
        self.id
    }

    fn desc_addr(&self) -> u64 {
        self.queue_pfn as u64 * self.guest_page_size as u64
    }

    fn read_disk(&self, addr: u64) -> u64 {
        self.disk[addr as usize] as u64
    }

    fn write_disk(&mut self, addr: u64, value: u64) {
        self.disk[addr as usize] = value as u8
    }

    /// Access the disk via virtio. This is an associated function which takes a `cpu` object to
    /// read and write with a memory directly (DMA).
    pub fn disk_access(cpu: &mut Cpu) -> Result<(), Exception> {
        // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1460002
        // "Used Buffer Notification
        //     - bit 0 - the interrupt was asserted because the device has used a buffer in at
        //     least one of the active virtual queues."
        cpu.bus.virtio.interrupt_status |= 0x1;

        // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-230005
        // "Each virtqueue can consist of up to 3 parts:
        //     Descriptor Area - used for describing buffers
        //     Driver Area - extra data supplied by driver to the device
        //     Device Area - extra data supplied by device to driver"
        //
        // https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/virtio_disk.c#L101-L103
        //     desc = pages -- num * VirtqDesc
        //     avail = pages + 0x40 -- 2 * uint16, then num * uint16
        //     used = pages + 4096 -- 2 * uint16, then num * vRingUsedElem
        //
        // The actual descriptors (16 bytes each).
        let desc_addr = cpu.bus.virtio.desc_addr();
        // A ring of available descriptor heads with free-running index.
        let avail_addr = cpu.bus.virtio.desc_addr() + 0x40;
        // A ring of used descriptor heads with free-running index.
        let used_addr = cpu.bus.virtio.desc_addr() + 4096;

        // 2.6.6 The Virtqueue Available Ring
        // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-380006
        // struct virtq_avail {
        //   #define VIRTQ_AVAIL_F_NO_INTERRUPT 1
        //   le16 flags;
        //   le16 idx;
        //   le16 ring[ /* Queue Size */ ];
        //   le16 used_event; /* Only if VIRTIO_F_EVENT_IDX */
        // };
        //
        // https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/virtio_disk.c#L230-L234
        // "avail[0] is flags
        //  avail[1] tells the device how far to look in avail[2...].
        //  avail[2...] are desc[] indices the device should process.
        //  we only tell device the first index in our chain of descriptors."
        let offset = cpu.bus.read(avail_addr.wrapping_add(1), HALFWORD)?;
        let index = cpu.bus.read(
            avail_addr.wrapping_add(offset % QUEUE_SIZE).wrapping_add(2),
            HALFWORD,
        )?;

        // First descriptor.
        let desc0 = VirtqDesc::new(cpu, desc_addr + VRING_DESC_SIZE * index)?;

        // Second descriptor.
        let desc1 = VirtqDesc::new(cpu, desc_addr + VRING_DESC_SIZE * desc0.next)?;

        // Third descriptor address.
        let desc2_addr = cpu
            .bus
            .read(desc_addr + VRING_DESC_SIZE * desc1.next, DOUBLEWORD)?;
        // Tell success.
        cpu.bus.write(desc2_addr, 0, BYTE)?;

        // 5.2.6 Device Operation
        // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2500006
        // struct virtio_blk_req {
        //   le32 type;
        //   le32 reserved;
        //   le64 sector;
        //   u8 data[][512];
        //   u8 status;
        // };
        let sector = cpu.bus.read(desc0.addr.wrapping_add(8), DOUBLEWORD)?;

        // Write to a device if the second bit of `flags` is set.
        match (desc1.flags & 2) == 0 {
            true => {
                // Read memory data and write it to a disk directly (DMA).
                for i in 0..desc1.len {
                    let data = cpu.bus.read(desc1.addr + i, BYTE)?;
                    cpu.bus.virtio.write_disk(sector * SECTOR_SIZE + i, data);
                }
            }
            false => {
                // Read disk data and write it to memory directly (DMA).
                for i in 0..desc1.len {
                    let data = cpu.bus.virtio.read_disk(sector * SECTOR_SIZE + i);
                    cpu.bus.write(desc1.addr + i, data, BYTE)?;
                }
            }
        };

        // 2.6.8 The Virtqueue Used Ring
        // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-430008
        // struct virtq_used {
        //   #define VIRTQ_USED_F_NO_NOTIFY 1
        //   le16 flags;
        //   le16 idx;
        //   struct virtq_used_elem ring[ /* Queue Size */];
        //   le16 avail_event; /* Only if VIRTIO_F_EVENT_IDX */
        // };
        let new_id = cpu.bus.virtio.get_new_id();
        cpu.bus
            .write(used_addr.wrapping_add(2), new_id % QUEUE_SIZE, HALFWORD)?;
        Ok(())
    }
}
