use crate::cache::{Cache, CacheCommon};
use std::{
    convert::TryFrom,
    iter::FromIterator,
    marker::PhantomData,
    mem,
    ops::{Deref, Range},
};
use wgpu::util::DeviceExt;

pub struct AlignedBufferCache<T> {
    inner: BufferCache<u8>,
    alignment: u16,
    _marker: PhantomData<T>,
}

impl<T> Deref for AlignedBufferCache<T> {
    type Target = Option<wgpu::Buffer>;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

impl<T> AlignedBufferCache<T> {
    pub fn new(buffer_usage: wgpu::BufferUsage, alignment: u16) -> Self {
        Self {
            inner: BufferCache::new(buffer_usage),
            alignment,
            _marker: PhantomData,
        }
    }
}

impl<T> CacheCommon for AlignedBufferCache<T>
where
    T: bytemuck::Pod,
{
    type Key = Range<u64>;

    fn update(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        self.inner.update(device, encoder);
    }

    fn clear(&mut self) {
        self.inner.clear()
    }
}

impl<T, I> Cache<I> for AlignedBufferCache<T>
where
    I: IntoIterator<Item = T>,
    T: bytemuck::Pod,
{
    fn append(&mut self, vals: I) -> Self::Key {
        let vals: Vec<T> = vals.into_iter().collect();

        let out = self
            .inner
            .append(bytemuck::cast_slice(&vals[..]).iter().copied());
        self.inner.append(
            std::iter::repeat(0).take(
                self.alignment as usize - (self.inner.len() % self.alignment as u64) as usize,
            ),
        );

        out
    }
}

pub struct BufferCache<T> {
    unwritten: Vec<T>,
    buffer: Option<wgpu::Buffer>,
    buffer_usage: wgpu::BufferUsage,
    buffer_len: u64,
}

impl<T> BufferCache<T> {
    pub fn new(buffer_usage: wgpu::BufferUsage) -> Self {
        Self {
            unwritten: Vec::new(),
            buffer: None,
            buffer_usage: buffer_usage | wgpu::BufferUsage::COPY_SRC,
            buffer_len: 0,
        }
    }

    pub fn len(&self) -> u64 {
        self.unwritten.len() as u64 + self.buffer_len
    }

    pub fn append_many<
        Int: TryFrom<u64>,
        O: FromIterator<Result<Range<Int>, <Int as TryFrom<u64>>::Error>>,
        Outer: IntoIterator<Item = Inner>,
        Inner: IntoIterator<Item = T>,
    >(
        &mut self,
        vals: Outer,
    ) -> (Range<u64>, O) {
        let start = self.len();
        let out = O::from_iter(vals.into_iter().map(|inner| {
            let start = self.len();
            self.unwritten.extend(inner);
            let end = self.len();

            Ok(Int::try_from(start)?..Int::try_from(end)?)
        }));
        let end = self.len();

        (start..end, out)
    }
}

impl<T> Deref for BufferCache<T> {
    type Target = Option<wgpu::Buffer>;

    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl<T> CacheCommon for BufferCache<T>
where
    T: bytemuck::Pod,
{
    type Key = Range<u64>;

    // TODO: Reuse the same buffer for transferring the data to the GPU, we don't need a new one every time.
    //       Really, this should be something like a `Vec` where it automatically resizes to a power of two.
    fn update(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        if !self.unwritten.is_empty() {
            if let Some(buffer) = &self.buffer {
                let start_of_free = self.buffer_len * mem::size_of::<T>() as u64;

                let new_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: start_of_free + (self.unwritten.len() * mem::size_of::<T>()) as u64,
                    usage: self.buffer_usage | wgpu::BufferUsage::COPY_DST,
                    mapped_at_creation: true,
                });

                new_buffer
                    .slice(start_of_free as u64..)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytemuck::cast_slice(&self.unwritten));
                new_buffer.unmap();

                encoder.copy_buffer_to_buffer(buffer, 0, &new_buffer, 0, start_of_free);

                self.buffer = Some(new_buffer);
            } else {
                self.buffer = Some(
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: bytemuck::cast_slice(&self.unwritten),
                        usage: self.buffer_usage,
                    }),
                );
            }

            self.buffer_len += self.unwritten.len() as u64;
            self.unwritten.clear();
        }
    }

    fn clear(&mut self) {
        self.unwritten.clear();
        self.buffer_len = 0;
    }
}

impl<T, I> Cache<I> for BufferCache<T>
where
    I: IntoIterator<Item = T>,
    T: bytemuck::Pod,
{
    fn append(&mut self, vals: I) -> Self::Key {
        let start = self.unwritten.len() as u64 + self.buffer_len;
        self.unwritten.extend(vals);
        let end = self.unwritten.len() as u64 + self.buffer_len;

        if start == end {
            0..0
        } else {
            start..end
        }
    }
}
