//! Texture image creation and sub-rectangle staging uploads.
//!
//! Because [`crate::create_texture`] has no access to a
//! [`crate::state::RenderState`], the GPU image is allocated lazily on the
//! first upload and cached in the state's texture table. Every upload leaves
//! the image in `SHADER_READ_ONLY_OPTIMAL`, which is the layout the pipelines
//! sample.
#![allow(unsafe_code)]

use std::time::Instant;

use ash::vk;

use api::error::RenderError;
use api::geometry::PixelRegion;

use crate::UploadTimings;
use crate::collections::{Map, Set, set_with_capacity};
use crate::texture::{Texture, TextureDesc, TextureFormat};

use super::VulkanDevice;
use super::shaders::SOURCE_DESCRIPTOR;

/// GPU image, view and descriptor set for one texture handle.
pub(crate) struct VulkanTexture {
    /// Descriptor the texture was created with.
    pub(crate) desc: TextureDesc,
    /// Backing device image.
    pub(crate) image: vk::Image,
    /// Sampled image view.
    pub(crate) view: vk::ImageView,
    /// Device memory backing [`Self::image`].
    pub(crate) memory: vk::DeviceMemory,
    /// Descriptor set bound for textured draws.
    pub(crate) descriptor_set: vk::DescriptorSet,
    /// Whether the image has received data and is sampleable.
    pub(crate) uploaded: bool,
}

impl VulkanTexture {
    /// Destroys the view, image, memory and descriptor set.
    ///
    /// The caller makes sure the device holds no work that samples the image
    /// before this runs: a texture outlives the draw that last read it until
    /// the caller retires it, and the caller waits for the device at the point
    /// where that draw has completed. With no command buffer referencing them,
    /// the handles are unbound and the memory goes back to the allocator. A
    /// descriptor set the device refuses to take back leaves the pool one set
    /// short, so the failure is reported: a set that silently disappears is a
    /// pool that runs dry with nothing in the log.
    pub(crate) fn destroy(&self, device: &ash::Device, descriptor_pool: vk::DescriptorPool) {
        // SAFETY: all handles were created by `device`; the caller guarantees
        // no submitted work still references them.
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
            if let Err(error) = device.free_descriptor_sets(descriptor_pool, &[self.descriptor_set])
            {
                log::warn!(
                    "platform.render: a freed texture kept its descriptor set, so the pool \
                     is one set short: {error}"
                );
            }
        }
    }
}

/// Persistent host-visible staging buffer reused by one upload slot.
///
/// The buffer grows geometrically and is only recreated when an upload needs
/// more space or after sustained small usage. Upload slots keep their buffer
/// persistently mapped for its lifetime so each batch copies directly into
/// host-visible memory without per-batch map/unmap calls. The owning
/// [`UploadSlot`] waits its fence before the buffer is touched, so the old
/// buffer is destroyed without a further wait.
pub(crate) struct StagingBuffer {
    /// The host-visible buffer; null until the first reserve.
    pub(crate) buffer: vk::Buffer,
    /// Memory backing [`Self::buffer`]; null until the first reserve.
    pub(crate) memory: vk::DeviceMemory,
    /// Persistently mapped base address of [`Self::memory`], when mapped.
    ///
    /// Stored as an address so the state stays free of raw pointers; `None`
    /// when the buffer is empty or unmapped.
    mapped: Option<usize>,
    capacity: vk::DeviceSize,
    /// Consecutive reserves at or below [`STAGING_SHRINK_TARGET_MAX`] while
    /// capacity exceeds [`STAGING_SHRINK_THRESHOLD`].
    small_reserves: u32,
}

impl Default for StagingBuffer {
    fn default() -> Self {
        Self {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: None,
            capacity: 0,
            small_reserves: 0,
        }
    }
}

/// Host-visible staging capacity above which a buffer is eligible to shrink
/// after sustained small uploads, so a single loading burst does not
/// permanently pin tens of megabytes of host-visible RAM.
const STAGING_SHRINK_THRESHOLD: vk::DeviceSize = 8 * 1024 * 1024;

/// Maximum upload size that counts toward shrinking an oversized staging
/// buffer (hysteresis gap below [`STAGING_SHRINK_THRESHOLD`]).
const STAGING_SHRINK_TARGET_MAX: vk::DeviceSize = 4 * 1024 * 1024;

/// Consecutive small reserves required before shrinking an oversized staging
/// buffer, so a multi-frame loading burst does not reallocate on every frame
/// while still reclaiming oversized buffers promptly.
const STAGING_SHRINK_DELAY_RESERVES: u32 = 8;

/// Target maximum byte size per staged upload chunk.
///
/// Large menu-loading bursts (such as Save/Load thumbnails) can upload over
/// 100 MiB of textures in one frame; chunking keeps each upload slot's
/// persistently mapped staging buffer bounded to ~8 MiB instead of pinning
/// 256+ MiB per slot in host-visible RAM, and overlaps host copies with GPU
/// transfers across the upload ring.
const MAX_UPLOAD_CHUNK_BYTES: usize = 8 * 1024 * 1024;

impl StagingBuffer {
    /// Destroys the staging buffer and its memory, if any.
    pub(crate) fn destroy(&self, device: &ash::Device) {
        if self.buffer == vk::Buffer::null() {
            return;
        }
        // SAFETY: both handles were created by `device` and the caller
        // guarantees no in-flight work still references them.
        unsafe {
            if self.mapped.is_some() {
                device.unmap_memory(self.memory);
            }
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
        }
    }

    /// Destroys the staging buffer and resets this handle to empty so its
    /// host-visible memory is returned to the allocator immediately.
    pub(crate) fn release(&mut self, device: &ash::Device) {
        self.destroy(device);
        self.buffer = vk::Buffer::null();
        self.memory = vk::DeviceMemory::null();
        self.mapped = None;
        self.capacity = 0;
        self.small_reserves = 0;
    }

    /// Grows (or shrinks after sustained small usage) the buffer to hold at
    /// least `size` bytes, leaving its memory unmapped.
    ///
    /// `usage` is the buffer usage the caller will use it for. The caller
    /// guarantees no GPU work still accesses the old buffer; it is therefore
    /// destroyed without a wait.
    pub(crate) fn reserve(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<(), RenderError> {
        self.reserve_with_mapping(device, memory_properties, size, usage, false)
    }

    /// Grows (or shrinks after sustained small usage) the buffer to hold at
    /// least `size` bytes and keeps its memory persistently mapped.
    fn reserve_mapped(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<(), RenderError> {
        self.reserve_with_mapping(device, memory_properties, size, usage, true)
    }

    /// Updates the shrink hysteresis counter and reports whether an oversized
    /// buffer has seen enough consecutive small reserves to shrink.
    fn should_shrink_after_reserve(&mut self, size: vk::DeviceSize) -> bool {
        if self.capacity > STAGING_SHRINK_THRESHOLD && size <= STAGING_SHRINK_TARGET_MAX {
            self.small_reserves = self.small_reserves.saturating_add(1);
            self.small_reserves >= STAGING_SHRINK_DELAY_RESERVES
        } else {
            self.small_reserves = 0;
            false
        }
    }

    /// Allocates or reuses the staging buffer for `size` bytes, optionally
    /// keeping the allocation persistently mapped.
    fn reserve_with_mapping(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        persistent_map: bool,
    ) -> Result<(), RenderError> {
        let should_shrink = self.should_shrink_after_reserve(size);
        let mapping_ready = !persistent_map || self.mapped.is_some();
        if self.buffer != vk::Buffer::null()
            && self.capacity >= size
            && !should_shrink
            && mapping_ready
        {
            return Ok(());
        }
        let capacity = staging_capacity(size)?;
        let (buffer, memory) = create_staging_buffer(device, memory_properties, capacity, usage)?;
        let mapped = if persistent_map {
            // SAFETY: `memory` was allocated with `HOST_VISIBLE | HOST_COHERENT`
            // and at least `capacity` bytes; the mapping stays valid until
            // `destroy`.
            let ptr = match unsafe {
                device.map_memory(memory, 0, capacity, vk::MemoryMapFlags::empty())
            } {
                Ok(ptr) => ptr,
                Err(error) => {
                    // SAFETY: both handles are owned here and unused elsewhere.
                    unsafe {
                        device.destroy_buffer(buffer, None);
                        device.free_memory(memory, None);
                    }
                    return Err(RenderError::Upload {
                        message: format!("failed to map the staging buffer: {error}"),
                    });
                }
            };
            Some(ptr as usize)
        } else {
            None
        };
        self.destroy(device);
        self.buffer = buffer;
        self.memory = memory;
        self.mapped = mapped;
        self.capacity = capacity;
        self.small_reserves = 0;
        Ok(())
    }

    /// Copies each upload's pixels into the persistently mapped buffer at its
    /// precomputed offset.
    ///
    /// `offsets` must be the layout returned by [`staging_layout`] for
    /// `uploads`.
    fn write(
        &self,
        total: usize,
        uploads: &[TextureUpload<'_>],
        offsets: &[usize],
    ) -> Result<(), RenderError> {
        let Some(mapped) = self.mapped else {
            return Err(RenderError::Upload {
                message: "the staging buffer is not mapped".to_owned(),
            });
        };
        if (total as vk::DeviceSize) > self.capacity {
            return Err(RenderError::Upload {
                message: format!(
                    "staged upload of {total} bytes exceeds the {}-byte staging buffer",
                    self.capacity
                ),
            });
        }
        let base = mapped as *mut u8;
        for (upload, &offset) in uploads.iter().zip(offsets) {
            // SAFETY: the offsets are the consecutive pixel lengths summed by
            // `staging_layout`, so `base + offset .. + pixels.len()` lies
            // inside `total <= self.capacity` mapped bytes, and the slot's
            // fence was waited on before `write` so the GPU is not reading it.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    upload.pixels.as_ptr(),
                    base.add(offset),
                    upload.pixels.len(),
                );
            }
        }
        Ok(())
    }
}

/// Number of independently synchronised texture-upload slots.
///
/// Two frames may be presented concurrently, so a ring of two could stall the
/// first upload after a frame boundary on a fence the present loop has not
/// reaped yet. A third slot absorbs that boundary, and three staging buffers
/// is a bounded amount of host memory.
pub(crate) const UPLOAD_SLOTS: usize = 3;

/// One independently synchronised upload slot.
struct UploadSlot {
    /// Host-visible staging buffer reused by this slot.
    staging: StagingBuffer,
    /// Pool owning [`Self::command`].
    pool: vk::CommandPool,
    /// Command buffer one upload batch is recorded into.
    command: vk::CommandBuffer,
    /// Signalled when this slot's last submit completes.
    fence: vk::Fence,
    /// Whether [`Self::fence`] still guards unfinished GPU work.
    in_flight: bool,
    /// Submission epoch of the work guarded by [`Self::fence`].
    epoch: u64,
    /// Number of batches submitted from this slot; a diagnostic counter.
    #[cfg(test)]
    submits: u64,
}

/// Round-robin ring of upload slots.
///
/// Each slot stages and submits one batch independently, so a batch only waits
/// on the GPU when its slot is reused, never right after it is submitted. The
/// ring is caller-owned state on the device; there are no globals.
pub(crate) struct UploadRing {
    slots: Vec<UploadSlot>,
    next: usize,
    /// Monotonic counter incremented on every accepted upload submission.
    submitted_epoch: u64,
    /// The costs of the most recent batch, measured on the thread that made
    /// it. `None` until the first batch.
    pub(crate) last: Option<UploadTimings>,
}

impl UploadRing {
    /// Creates `count` slots whose fences start signalled.
    pub(crate) fn new(
        device: &ash::Device,
        queue_family_index: u32,
        count: usize,
    ) -> Result<Self, RenderError> {
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            match create_upload_slot(device, queue_family_index) {
                Ok(slot) => slots.push(slot),
                Err(error) => {
                    for slot in &slots {
                        destroy_upload_slot(device, slot);
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            slots,
            next: 0,
            submitted_epoch: 0,
            last: None,
        })
    }

    /// Returns the epoch of the most recently submitted upload batch.
    pub(crate) fn submitted_epoch(&self) -> u64 {
        self.submitted_epoch
    }

    /// Polls in-flight upload fences without blocking and returns the highest
    /// upload epoch guaranteed to have completed on the GPU.
    pub(crate) fn safe_epoch(&mut self, device: &ash::Device) -> u64 {
        for slot in &mut self.slots {
            if !slot.in_flight {
                continue;
            }
            // SAFETY: `slot.fence` was created by `device`.
            match unsafe { device.get_fence_status(slot.fence) } {
                Ok(true) => slot.in_flight = false,
                Ok(false) => {}
                Err(error) => {
                    log::warn!("platform.render: failed to poll an upload fence: {error}");
                }
            }
        }
        self.slots
            .iter()
            .filter(|slot| slot.in_flight)
            .map(|slot| slot.epoch.saturating_sub(1))
            .min()
            .unwrap_or(self.submitted_epoch)
    }

    /// Destroys every slot's staging buffer, command pool and fence.
    ///
    /// The caller guarantees every slot's fence has been waited.
    pub(crate) fn destroy(&self, device: &ash::Device) {
        for slot in &self.slots {
            destroy_upload_slot(device, slot);
        }
    }

    /// Releases the host-visible staging buffer of every slot whose GPU work
    /// has completed, without blocking on in-flight uploads.
    pub(crate) fn shrink_staging_buffers(&mut self, device: &ash::Device) {
        for slot in &mut self.slots {
            if slot.in_flight {
                // SAFETY: `slot.fence` was created by `device`.
                let signaled = unsafe { device.get_fence_status(slot.fence) }.unwrap_or(false);
                if !signaled {
                    continue;
                }
                slot.in_flight = false;
            }
            slot.staging.release(device);
        }
    }

    /// Returns the number of upload batches submitted through this ring.
    ///
    /// This is a diagnostic: a caller can assert that a frame's N texture
    /// uploads were merged into one batch instead of N.
    #[cfg(test)]
    pub(crate) fn submits(&self) -> u64 {
        self.slots.iter().map(|slot| slot.submits).sum()
    }

    /// Waits for every in-flight upload to complete.
    ///
    /// The wait is the one the read path makes: a draw samples the textures
    /// the batches wrote, so it waits for the batches before it draws.
    pub(crate) fn wait_all(&mut self, device: &ash::Device) -> Result<(), RenderError> {
        for slot in &mut self.slots {
            if !slot.in_flight {
                continue;
            }
            // SAFETY: the fence guards this slot's last submit.
            unsafe { device.wait_for_fences(&[slot.fence], true, u64::MAX) }.map_err(|error| {
                RenderError::Upload {
                    message: format!("failed to wait for an upload: {error}"),
                }
            })?;
            slot.in_flight = false;
        }
        Ok(())
    }

    /// Takes a slot for the next batch and resets its command pool.
    ///
    /// The wait is the staging buffer's: the host writes the next batch's
    /// pixels into the slot's buffer, and the device may still be reading it.
    /// A slot that is not in flight holds a buffer the device has finished
    /// with, so it is taken first; when every slot is busy the round-robin
    /// one is taken and waited for.
    ///
    /// Two batches that write the same texture do not wait for each other
    /// here: a queue runs its submissions in the order they were made, so the
    /// second batch's layout transition runs after the first batch's, and the
    /// read path waits for both before it samples the texture.
    fn acquire(&mut self, device: &ash::Device) -> Result<(&mut UploadSlot, f64), RenderError> {
        let index = self.free_slot().unwrap_or(self.next);
        self.next = (index + 1) % self.slots.len();
        let slot = &mut self.slots[index];
        let mut wait_ms = 0.0;
        if slot.in_flight {
            let started = Instant::now();
            // SAFETY: the fence guards this slot's last submit.
            unsafe { device.wait_for_fences(&[slot.fence], true, u64::MAX) }.map_err(|error| {
                RenderError::Upload {
                    message: format!("failed to wait for an upload slot: {error}"),
                }
            })?;
            wait_ms = started.elapsed().as_secs_f64() * 1000.0;
            slot.in_flight = false;
        }
        // SAFETY: the slot's fence has been waited, so its pool is idle.
        unsafe {
            device
                .reset_command_pool(slot.pool, vk::CommandPoolResetFlags::empty())
                .map_err(|error| RenderError::Upload {
                    message: format!("failed to reset an upload command pool: {error}"),
                })?;
        }
        Ok((slot, wait_ms))
    }

    /// The first slot that is not in flight, in round-robin order.
    fn free_slot(&self) -> Option<usize> {
        let count = self.slots.len();
        for offset in 0..count {
            let index = (self.next + offset) % count;
            if !self.slots[index].in_flight {
                return Some(index);
            }
        }
        None
    }
}

/// Creates one upload slot with a signalled fence.
fn create_upload_slot(
    device: &ash::Device,
    queue_family_index: u32,
) -> Result<UploadSlot, RenderError> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .queue_family_index(queue_family_index)
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
    // SAFETY: `device` is live and the pool info is valid.
    let pool = unsafe { device.create_command_pool(&pool_info, None) }.map_err(|error| {
        RenderError::DeviceUnavailable {
            message: format!("failed to create an upload command pool: {error}"),
        }
    })?;

    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: the pool was just created.
    let command = match unsafe { device.allocate_command_buffers(&allocate_info) } {
        Ok(mut buffers) if !buffers.is_empty() => buffers.remove(0),
        Ok(_) => {
            // SAFETY: the pool is owned here.
            unsafe { device.destroy_command_pool(pool, None) };
            return Err(RenderError::OutOfMemory {
                message: "the upload pool returned no command buffer".to_owned(),
            });
        }
        Err(error) => {
            // SAFETY: the pool is owned here.
            unsafe { device.destroy_command_pool(pool, None) };
            return Err(RenderError::OutOfMemory {
                message: format!("failed to allocate an upload command buffer: {error}"),
            });
        }
    };

    // SAFETY: `device` is live.
    let fence = match unsafe {
        device.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )
    } {
        Ok(fence) => fence,
        Err(error) => {
            // SAFETY: the pool is owned here.
            unsafe { device.destroy_command_pool(pool, None) };
            return Err(RenderError::DeviceUnavailable {
                message: format!("failed to create an upload fence: {error}"),
            });
        }
    };

    Ok(UploadSlot {
        staging: StagingBuffer::default(),
        pool,
        command,
        fence,
        in_flight: false,
        epoch: 0,
        #[cfg(test)]
        submits: 0,
    })
}

/// Destroys one upload slot's fence, command pool and staging buffer.
fn destroy_upload_slot(device: &ash::Device, slot: &UploadSlot) {
    // SAFETY: all handles were created by `device`; the caller guarantees the
    // slot's fence has been waited.
    unsafe {
        device.destroy_fence(slot.fence, None);
        device.destroy_command_pool(slot.pool, None);
    }
    slot.staging.destroy(device);
}

/// Returns the capacity to allocate for a staging buffer holding at least
/// `required` bytes, rounded up to a power of two.
fn staging_capacity(required: vk::DeviceSize) -> Result<vk::DeviceSize, RenderError> {
    required
        .checked_next_power_of_two()
        .map(|capacity| capacity.max(64))
        .ok_or_else(|| RenderError::Upload {
            message: "the staging buffer size overflows the address space".to_owned(),
        })
}

/// Returns the Vulkan format used for a [`TextureFormat`].
///
/// The pipeline composites in the encoded space, so the colour channels are
/// stored and sampled without a transfer function: the bytes the engine
/// writes are the bytes the blend reads.
pub(crate) fn image_format(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::Rgba8Srgb | TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::R8Unorm => vk::Format::R8_UNORM,
    }
}

/// Maps the component swizzle used for R8 coverage textures.
///
/// An `R8_UNORM` image is broadcast to `(1, 1, 1, R)` so glyph coverage lands
/// in the alpha channel and the tint carries the text colour.
fn component_mapping(format: TextureFormat) -> vk::ComponentMapping {
    match format {
        TextureFormat::R8Unorm => vk::ComponentMapping {
            r: vk::ComponentSwizzle::ONE,
            g: vk::ComponentSwizzle::ONE,
            b: vk::ComponentSwizzle::ONE,
            a: vk::ComponentSwizzle::R,
        },
        _ => vk::ComponentMapping::default(),
    }
}

/// Creates the GPU image, view and descriptor set for `desc`.
pub(crate) fn create_texture(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    desc: TextureDesc,
) -> Result<VulkanTexture, RenderError> {
    let format = image_format(desc.format);
    let mip_levels = desc.mip_levels.max(1);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: desc.width,
            height: desc.height,
            depth: 1,
        })
        .mip_levels(mip_levels)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);

    // SAFETY: `image_info` borrows a local valid for the call.
    let image = unsafe { device.create_image(&image_info, None) }.map_err(|error| {
        RenderError::OutOfMemory {
            message: format!("failed to create a texture image: {error}"),
        }
    })?;

    // SAFETY: `image` was just created by `device`.
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let memory_type = match find_memory_type(
        memory_properties,
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    ) {
        Ok(memory_type) => memory_type,
        Err(error) => {
            // SAFETY: `image` is owned here and not used elsewhere yet.
            unsafe { device.destroy_image(image, None) };
            return Err(error);
        }
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    // SAFETY: `allocate_info` is valid; the memory is freed with the image.
    let memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|error| {
        // SAFETY: `image` is owned here and not used elsewhere yet.
        unsafe { device.destroy_image(image, None) };
        RenderError::OutOfMemory {
            message: format!("failed to allocate texture memory: {error}"),
        }
    })?;

    // SAFETY: `memory` was allocated for `image`.
    if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
        // SAFETY: both handles are owned here.
        unsafe {
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(RenderError::OutOfMemory {
            message: format!("failed to bind texture memory: {error}"),
        });
    }

    let subresource_range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(mip_levels)
        .base_array_layer(0)
        .layer_count(1);
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .components(component_mapping(desc.format))
        .subresource_range(subresource_range);
    // SAFETY: `image` was created with a compatible format.
    let view = match unsafe { device.create_image_view(&view_info, None) } {
        Ok(view) => view,
        Err(error) => {
            // SAFETY: both handles are owned here.
            unsafe {
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
            return Err(RenderError::OutOfMemory {
                message: format!("failed to create a texture image view: {error}"),
            });
        }
    };

    let set_layouts = [descriptor_set_layout];
    let allocate_sets = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(descriptor_pool)
        .set_layouts(&set_layouts);
    // SAFETY: `allocate_sets` borrows locals valid for the call.
    let descriptor_set = match unsafe { device.allocate_descriptor_sets(&allocate_sets) } {
        Ok(mut sets) => match sets.pop() {
            Some(set) => set,
            None => {
                // SAFETY: both handles are owned here.
                unsafe {
                    device.destroy_image_view(view, None);
                    device.destroy_image(image, None);
                    device.free_memory(memory, None);
                }
                return Err(RenderError::OutOfMemory {
                    message: "the descriptor pool returned no descriptor set".to_owned(),
                });
            }
        },
        Err(error) => {
            // SAFETY: both handles are owned here.
            unsafe {
                device.destroy_image_view(view, None);
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
            return Err(RenderError::OutOfMemory {
                message: format!("failed to allocate a texture descriptor set: {error}"),
            });
        }
    };

    let image_info = vk::DescriptorImageInfo::default()
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .image_view(view)
        .sampler(sampler);
    let image_infos = [image_info];
    let write = vk::WriteDescriptorSet::default()
        .dst_set(descriptor_set)
        .dst_binding(SOURCE_DESCRIPTOR.binding)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_infos);
    // SAFETY: the descriptor set is not in use and the image info is valid.
    unsafe { device.update_descriptor_sets(&[write], &[]) };

    Ok(VulkanTexture {
        desc,
        image,
        view,
        memory,
        descriptor_set,
        uploaded: false,
    })
}

/// The device, memory and queue an upload batch is staged and submitted on.
#[derive(Clone, Copy)]
pub(crate) struct UploadContext<'a> {
    /// Logical device that owns the images and staging buffers.
    pub(crate) device: &'a ash::Device,
    /// Memory properties that pick the staging buffer's memory type.
    pub(crate) memory_properties: &'a vk::PhysicalDeviceMemoryProperties,
    /// Queue the batch is submitted to.
    pub(crate) queue: vk::Queue,
}

/// Copies a tightly packed sub-rectangle of `pixels` into `texture`.
///
/// The batch is staged and submitted to a free slot of `ring` without waiting.
/// `key` identifies `texture` in the state's texture table.
pub(crate) fn upload(
    context: &UploadContext<'_>,
    ring: &mut UploadRing,
    texture: &mut VulkanTexture,
    key: u64,
    region: PixelRegion,
    pixels: &[u8],
) -> Result<(), RenderError> {
    let total = pixels.len();
    if total == 0 {
        return Ok(());
    }
    // A single upload is a one-element batch, so it shares the batch's
    // staging, recording and image-layout rules.
    let uploads = [TextureUpload {
        key,
        region,
        pixels,
    }];
    let offsets = [0usize];
    let first_upload = !texture.uploaded;
    let batch = StageBatch {
        uploads: &uploads,
        offsets: &offsets,
        total,
    };
    let result = submit_batch(context, ring, batch, |command_buffer, staging_buffer| {
        record_copy(
            context,
            CopyRecord {
                command_buffer,
                staging_buffer,
                texture,
                region,
                staging_offset: 0,
                first_upload,
            },
        );
        Ok(())
    });
    if result.is_ok() {
        texture.uploaded = true;
    }
    result
}

/// One texture update in a batch uploaded by [`upload_batch`].
///
/// `pixels` is tightly packed, row-major, for `region` in the destination
/// texture's format.
pub(crate) struct TextureUpload<'a> {
    /// Identity of the destination texture in the state's texture table.
    pub(crate) key: u64,
    /// Sub-rectangle to replace.
    pub(crate) region: PixelRegion,
    /// Tightly packed pixels for `region`.
    pub(crate) pixels: &'a [u8],
}

/// Copies a batch of sub-rectangles into their textures, splitting batches
/// larger than [`MAX_UPLOAD_CHUNK_BYTES`] into multiple ring submissions so
/// host-visible staging buffers stay bounded instead of pinning hundreds of
/// megabytes during loading bursts.
///
/// Every destination must already exist in `textures` and every `region` must
/// have been validated against its texture. Waiting happens in [`UploadRing`]
/// only when a slot is reused or when `wait_all` asks the present path for the
/// pixels.
pub(crate) fn upload_batch(
    context: &UploadContext<'_>,
    ring: &mut UploadRing,
    textures: &mut Map<u64, VulkanTexture>,
    uploads: &[TextureUpload<'_>],
) -> Result<(), RenderError> {
    if uploads.is_empty() {
        return Ok(());
    }

    let mut total_timings = UploadTimings {
        create_ms: 0.0,
        stage_ms: 0.0,
        submit_ms: 0.0,
        wait_ms: 0.0,
    };
    let mut start = 0usize;
    while start < uploads.len() {
        let mut end = start;
        let mut chunk_bytes = 0usize;
        while end < uploads.len() {
            let next_len = uploads[end].pixels.len();
            if end > start && chunk_bytes.saturating_add(next_len) > MAX_UPLOAD_CHUNK_BYTES {
                break;
            }
            chunk_bytes = chunk_bytes
                .checked_add(next_len)
                .ok_or_else(|| RenderError::Upload {
                    message: "the upload batch exceeds the host address space".to_owned(),
                })?;
            end += 1;
        }
        let chunk = &uploads[start..end];
        let (offsets, total) = staging_layout(chunk)?;
        // A texture repeated in one chunk is only in `UNDEFINED` for its first
        // copy; every later copy starts from the sampled layout the previous
        // transition left behind.
        let mut seen: Set<u64> = set_with_capacity(chunk.len());
        let batch = StageBatch {
            uploads: chunk,
            offsets: &offsets,
            total,
        };
        submit_batch(context, ring, batch, |command_buffer, staging_buffer| {
            for (upload, &offset) in chunk.iter().zip(&offsets) {
                let gpu = textures
                    .get_mut(&upload.key)
                    .ok_or_else(|| RenderError::Upload {
                        message: "texture state disappeared during upload".to_owned(),
                    })?;
                let first_upload = !gpu.uploaded && seen.insert(upload.key);
                record_copy(
                    context,
                    CopyRecord {
                        command_buffer,
                        staging_buffer,
                        texture: gpu,
                        region: upload.region,
                        staging_offset: offset as vk::DeviceSize,
                        first_upload,
                    },
                );
            }
            Ok(())
        })?;
        // The images stay sampleable if the submit failed: nothing reached the
        // queue, so their recorded transitions never ran.
        for upload in chunk {
            if let Some(gpu) = textures.get_mut(&upload.key) {
                gpu.uploaded = true;
            }
        }
        if let Some(last) = ring.last {
            total_timings.create_ms += last.create_ms;
            total_timings.stage_ms += last.stage_ms;
            total_timings.submit_ms += last.submit_ms;
            total_timings.wait_ms += last.wait_ms;
        }
        start = end;
    }
    ring.last = Some(total_timings);
    Ok(())
}

/// One upload batch's staging plan: the uploads, their staging offsets and the
/// total staged byte count.
struct StageBatch<'a> {
    /// Updates staged into the slot.
    uploads: &'a [TextureUpload<'a>],
    /// Staging offset of every entry in `uploads`.
    offsets: &'a [usize],
    /// Total staged byte count.
    total: usize,
}

/// Stages a batch in a free ring slot and submits it without waiting.
///
/// `record` receives the recording command buffer and the staging buffer it
/// must copy from. Nothing is submitted when `record` fails, so the slot's
/// fence stays signalled and the slot is reusable.
fn submit_batch<F>(
    context: &UploadContext<'_>,
    ring: &mut UploadRing,
    batch: StageBatch<'_>,
    record: F,
) -> Result<(), RenderError>
where
    F: FnOnce(vk::CommandBuffer, vk::Buffer) -> Result<(), RenderError>,
{
    let UploadContext {
        device,
        memory_properties,
        queue,
    } = *context;
    let StageBatch {
        uploads,
        offsets,
        total,
    } = batch;
    // The batch's costs are measured below. A batch that fails leaves no
    // measurement behind, so a reader never sees the costs of an earlier
    // batch in its place.
    ring.last = None;
    let next_epoch = ring.submitted_epoch.saturating_add(1);
    let (slot, wait_ms) = ring.acquire(device)?;
    let stage_started = Instant::now();
    slot.staging.reserve_mapped(
        device,
        memory_properties,
        total as vk::DeviceSize,
        vk::BufferUsageFlags::TRANSFER_SRC,
    )?;
    slot.staging.write(total, uploads, offsets)?;
    let stage_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    let staging_buffer = slot.staging.buffer;

    // SAFETY: the slot's pool was reset by `acquire` and the command buffer is
    // idle.
    unsafe {
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        device
            .begin_command_buffer(slot.command, &begin_info)
            .map_err(|error| RenderError::Upload {
                message: format!("failed to begin an upload command buffer: {error}"),
            })?;
    }

    // A recording failure leaves the command buffer unsubmitted, so the fence
    // stays signalled and the slot is reusable.
    record(slot.command, staging_buffer)?;

    let submit_started = Instant::now();
    // SAFETY: the command buffer is recording and was begun above. The fence
    // is reset only immediately before the submit so an earlier error leaves
    // it signalled and the slot reusable. A failed submit leaves the fence
    // unsignalled but `in_flight` false, so no later wait hangs on it.
    unsafe {
        device
            .end_command_buffer(slot.command)
            .map_err(|error| RenderError::Upload {
                message: format!("failed to end an upload command buffer: {error}"),
            })?;
        device
            .reset_fences(&[slot.fence])
            .map_err(|error| RenderError::Upload {
                message: format!("failed to reset an upload fence: {error}"),
            })?;
        let command_buffers = [slot.command];
        let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
        device
            .queue_submit(queue, &[submit], slot.fence)
            .map_err(|error| RenderError::Upload {
                message: format!("failed to submit an upload: {error}"),
            })?;
    }
    let submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;

    slot.epoch = next_epoch;
    slot.in_flight = true;
    #[cfg(test)]
    {
        slot.submits = slot.submits.saturating_add(1);
    }
    ring.submitted_epoch = next_epoch;
    // The batch's costs: the wait for the staging buffer, the write of the
    // pixels into it, and the device's copy command.
    ring.last = Some(UploadTimings {
        create_ms: 0.0,
        stage_ms,
        submit_ms,
        wait_ms,
    });
    Ok(())
}

/// Returns the staging offset of every upload and the total byte count.
///
/// Offsets are consecutive, starting at zero, so upload `i` occupies
/// `[offsets[i], offsets[i] + pixels.len())`. An overflow is a typed error
/// rather than a truncation.
fn staging_layout(uploads: &[TextureUpload<'_>]) -> Result<(Vec<usize>, usize), RenderError> {
    let mut offsets = Vec::with_capacity(uploads.len());
    let mut total: usize = 0;
    for upload in uploads {
        offsets.push(total);
        total = total
            .checked_add(upload.pixels.len())
            .ok_or_else(|| RenderError::Upload {
                message: "the upload batch exceeds the host address space".to_owned(),
            })?;
    }
    Ok((offsets, total))
}

/// One staged copy: which command buffer and staging buffer it records into
/// and the destination range it writes.
struct CopyRecord<'a> {
    /// Command buffer the copy is recorded into.
    command_buffer: vk::CommandBuffer,
    /// Staging buffer the pixels are read from.
    staging_buffer: vk::Buffer,
    /// Destination texture image.
    texture: &'a VulkanTexture,
    /// Sub-rectangle of `texture` to write.
    region: PixelRegion,
    /// Byte offset of this copy's pixels inside `staging_buffer`.
    staging_offset: vk::DeviceSize,
    /// Whether the image is still in its initial `UNDEFINED` layout.
    first_upload: bool,
}

/// Records the layout transitions and buffer-to-image copy for one upload.
fn record_copy(context: &UploadContext<'_>, copy: CopyRecord<'_>) {
    let UploadContext { device, .. } = *context;
    let CopyRecord {
        command_buffer,
        staging_buffer,
        texture,
        region,
        staging_offset,
        first_upload,
    } = copy;
    let (old_layout, src_access, src_stage) = if first_upload {
        (
            vk::ImageLayout::UNDEFINED,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TOP_OF_PIPE,
        )
    } else {
        (
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::AccessFlags::SHADER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        )
    };

    // SAFETY: the command buffer is recording and `texture` is live.
    unsafe {
        transition_image(
            device,
            command_buffer,
            texture,
            ImageTransition {
                old_layout,
                new_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                src_access,
                dst_access: vk::AccessFlags::TRANSFER_WRITE,
                src_stage,
                dst_stage: vk::PipelineStageFlags::TRANSFER,
            },
        );

        let subresource = vk::ImageSubresourceLayers::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .mip_level(0)
            .base_array_layer(0)
            .layer_count(1);
        let copy = vk::BufferImageCopy::default()
            .buffer_offset(staging_offset)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(subresource)
            .image_offset(vk::Offset3D {
                x: region.x as i32,
                y: region.y as i32,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: region.width,
                height: region.height,
                depth: 1,
            });
        device.cmd_copy_buffer_to_image(
            command_buffer,
            staging_buffer,
            texture.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[copy],
        );
        transition_image(
            device,
            command_buffer,
            texture,
            ImageTransition {
                old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                src_access: vk::AccessFlags::TRANSFER_WRITE,
                dst_access: vk::AccessFlags::SHADER_READ,
                src_stage: vk::PipelineStageFlags::TRANSFER,
                dst_stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            },
        );
    }
}

impl VulkanDevice {
    /// Copies a sub-rectangle of `pixels` into `tex`.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Upload`] when the texture cannot be created or
    /// the upload cannot be staged.
    pub(crate) fn upload_texture(
        &mut self,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        tex: &Texture,
        region: PixelRegion,
        pixels: &[u8],
    ) -> Result<(), RenderError> {
        let key = tex.key();
        let create_started = Instant::now();
        if !self.textures.contains_key(&key) {
            let gpu = self.create_texture_or_reap(memory_properties, *tex.desc())?;
            self.textures.insert(key, gpu);
        }
        let create_ms = create_started.elapsed().as_secs_f64() * 1000.0;

        let gpu = self
            .textures
            .get_mut(&key)
            .ok_or_else(|| RenderError::Upload {
                message: "texture state disappeared during upload".to_owned(),
            })?;
        let context = UploadContext {
            device: &self.device,
            memory_properties,
            queue: self.graphics_queue,
        };
        let result = upload(&context, &mut self.upload, gpu, key, region, pixels);
        if let Some(timings) = self.upload.last.as_mut() {
            timings.create_ms = create_ms;
        }
        result
    }

    /// Copies every upload's sub-rectangle into its texture as one batch.
    ///
    /// Every region is validated and every destination image created before
    /// the batch is staged, recorded and submitted to a free upload slot. The
    /// call does not wait for that submit; the present path waits for the
    /// uploads it is about to sample (or a later upload to the same texture
    /// waits when it reuses an in-flight slot).
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Upload`] when a region is out of bounds, the
    /// pixel slice does not match it or the batch cannot be staged.
    pub(crate) fn upload_textures(
        &mut self,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        uploads: &[crate::TextureUpload<'_>],
    ) -> Result<(), RenderError> {
        for upload in uploads {
            super::validate_upload(upload.texture.desc(), upload.region, upload.pixels)?;
        }

        let create_started = Instant::now();
        for upload in uploads {
            let key = upload.texture.key();
            if !self.textures.contains_key(&key) {
                let gpu = self.create_texture_or_reap(memory_properties, *upload.texture.desc())?;
                self.textures.insert(key, gpu);
            }
        }
        let create_ms = create_started.elapsed().as_secs_f64() * 1000.0;

        let batch: Vec<TextureUpload<'_>> = uploads
            .iter()
            .map(|upload| TextureUpload {
                key: upload.texture.key(),
                region: upload.region,
                pixels: upload.pixels,
            })
            .collect();

        let context = UploadContext {
            device: &self.device,
            memory_properties,
            queue: self.graphics_queue,
        };
        let result = upload_batch(&context, &mut self.upload, &mut self.textures, &batch);
        // The batch's own costs are recorded by the batch; the images the
        // batch writes into were created before it.
        if let Some(timings) = self.upload.last.as_mut() {
            timings.create_ms = create_ms;
        }
        result
    }
}

/// Creates a host-visible, host-coherent staging buffer.
fn create_staging_buffer(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    size: vk::DeviceSize,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory), RenderError> {
    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `buffer_info` borrows a local valid for the call.
    let buffer = unsafe { device.create_buffer(&buffer_info, None) }.map_err(|error| {
        RenderError::Upload {
            message: format!("failed to create the staging buffer: {error}"),
        }
    })?;

    // SAFETY: `buffer` was just created by `device`.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory_type = match find_memory_type(
        memory_properties,
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    ) {
        Ok(memory_type) => memory_type,
        Err(error) => {
            // SAFETY: `buffer` is owned here and not used elsewhere yet.
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(error);
        }
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    // SAFETY: `allocate_info` is valid.
    let memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|error| {
        // SAFETY: `buffer` is owned here.
        unsafe { device.destroy_buffer(buffer, None) };
        RenderError::OutOfMemory {
            message: format!("failed to allocate staging memory: {error}"),
        }
    })?;

    // SAFETY: `memory` was allocated for `buffer`.
    if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
        // SAFETY: both handles are owned here.
        unsafe {
            device.destroy_buffer(buffer, None);
            device.free_memory(memory, None);
        }
        return Err(RenderError::Upload {
            message: format!("failed to bind staging memory: {error}"),
        });
    }

    Ok((buffer, memory))
}

/// One image-memory barrier's layout, access and stage span.
struct ImageTransition {
    /// Layout the image is in before the barrier.
    old_layout: vk::ImageLayout,
    /// Layout the image is in after the barrier.
    new_layout: vk::ImageLayout,
    /// Accesses that must complete before the barrier.
    src_access: vk::AccessFlags,
    /// Accesses the barrier makes visible afterwards.
    dst_access: vk::AccessFlags,
    /// Pipeline stage the barrier waits on.
    src_stage: vk::PipelineStageFlags,
    /// Pipeline stage the barrier releases to.
    dst_stage: vk::PipelineStageFlags,
}

/// Emits an image-memory barrier that transitions `texture` between layouts.
fn transition_image(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    texture: &VulkanTexture,
    transition: ImageTransition,
) {
    let ImageTransition {
        old_layout,
        new_layout,
        src_access,
        dst_access,
        src_stage,
        dst_stage,
    } = transition;
    let subresource_range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(texture.desc.mip_levels.max(1))
        .base_array_layer(0)
        .layer_count(1);
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(texture.image)
        .subresource_range(subresource_range)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access);
    // SAFETY: `barrier` refers to a live image owned by `texture`.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

/// Finds a memory type satisfying `properties` among `type_bits`.
pub(crate) fn find_memory_type(
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    properties: vk::MemoryPropertyFlags,
) -> Result<u32, RenderError> {
    for index in 0..memory_properties.memory_type_count {
        let supported = type_bits & (1u32 << index) != 0;
        if supported
            && memory_properties.memory_types[index as usize]
                .property_flags
                .contains(properties)
        {
            return Ok(index);
        }
    }
    Err(RenderError::OutOfMemory {
        message: format!("no memory type supports {properties:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_layout_is_consecutive() {
        let pixels = [0u8; 3];
        let uploads = [
            TextureUpload {
                key: 1,
                region: PixelRegion::new(0, 0, 1, 1),
                pixels: &pixels[..1],
            },
            TextureUpload {
                key: 2,
                region: PixelRegion::new(0, 0, 1, 1),
                pixels: &pixels[..2],
            },
        ];
        let (offsets, total) = staging_layout(&uploads).expect("layout");
        assert_eq!(offsets, vec![0, 1]);
        assert_eq!(total, 3);
    }

    #[test]
    fn staging_capacity_rounds_up_to_a_power_of_two() {
        assert_eq!(staging_capacity(1).expect("capacity"), 64);
        assert_eq!(staging_capacity(64).expect("capacity"), 64);
        assert_eq!(staging_capacity(65).expect("capacity"), 128);
        assert!(staging_capacity(u64::MAX).is_err());
    }
}
