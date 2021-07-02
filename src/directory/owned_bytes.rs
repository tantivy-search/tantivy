use crate::directory::FileHandle;
use stable_deref_trait::StableDeref;
use std::convert::TryInto;
use std::mem;
use std::ops::{Deref, Range};
use std::sync::Arc;
use std::{fmt, io};

/// An OwnedBytes simply wraps an object that owns a slice of data and exposes
/// this data as a static slice.
///
/// The backing object is required to be `StableDeref`.
#[derive(Clone)]
pub struct OwnedBytes {
    data: &'static [u8],
    box_stable_deref: Arc<dyn Deref<Target = [u8]> + Sync + Send>,
}

impl FileHandle for OwnedBytes {
    fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
        Ok(self.slice(range))
    }
}

impl OwnedBytes {
    /// Creates an empty `OwnedBytes`.
    pub fn empty() -> OwnedBytes {
        OwnedBytes::new(&[][..])
    }

    /// Creates an `OwnedBytes` intance given a `StableDeref` object.
    pub fn new<T: StableDeref + Deref<Target = [u8]> + 'static + Send + Sync>(
        data_holder: T,
    ) -> OwnedBytes {
        let box_stable_deref = Arc::new(data_holder);
        let bytes: &[u8] = box_stable_deref.as_ref();
        let data = unsafe { mem::transmute::<_, &'static [u8]>(bytes.deref()) };
        OwnedBytes {
            data,
            box_stable_deref,
        }
    }

    /// creates a fileslice that is just a view over a slice of the data.
    pub fn slice(&self, range: Range<usize>) -> Self {
        OwnedBytes {
            data: &self.data[range],
            box_stable_deref: self.box_stable_deref.clone(),
        }
    }

    /// Returns the underlying slice of data.
    /// `Deref` and `AsRef` are also available.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        self.data
    }

    /// Returns the len of the slice.
    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Splits the OwnedBytes into two OwnedBytes `(left, right)`.
    pub fn split_from_end(self, right_len: usize) -> (OwnedBytes, OwnedBytes) {
        let left_len = self.len() - right_len;
        self.split(left_len)
    }

    /// Splits the OwnedBytes into two OwnedBytes `(left, right)`.
    ///
    /// Left will hold `split_len` bytes.
    ///
    /// This operation is cheap and does not require to copy any memory.
    /// On the other hand, both `left` and `right` retain a handle over
    /// the entire slice of memory. In other words, the memory will only
    /// be released when both left and right are dropped.
    pub fn split(self, split_len: usize) -> (OwnedBytes, OwnedBytes) {
        let right_box_stable_deref = self.box_stable_deref.clone();
        let left = OwnedBytes {
            data: &self.data[..split_len],
            box_stable_deref: self.box_stable_deref,
        };
        let right = OwnedBytes {
            data: &self.data[split_len..],
            box_stable_deref: right_box_stable_deref,
        };
        (left, right)
    }

    /// Returns true iff this `OwnedBytes` is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Drops the left most `advance_len` bytes.
    ///
    /// See also [.clip(clip_len: usize))](#method.clip).
    #[inline]
    pub fn advance(&mut self, advance_len: usize) {
        self.data = &self.data[advance_len..]
    }

    /// Reads an `u8` from the `OwnedBytes` and advance by one byte.
    pub fn read_u8(&mut self) -> u8 {
        assert!(!self.is_empty());

        let byte = self.as_slice()[0];
        self.advance(1);
        byte
    }

    /// Reads an `u64` encoded as little-endian from the `OwnedBytes` and advance by 8 bytes.
    pub fn read_u64(&mut self) -> u64 {
        assert!(self.len() > 7);

        let octlet: [u8; 8] = self.as_slice()[..8].try_into().unwrap();
        self.advance(8);
        u64::from_le_bytes(octlet)
    }
}

impl fmt::Debug for OwnedBytes {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // We truncate the bytes in order to make sure the debug string
        // is not too long.
        let bytes_truncated: &[u8] = if self.len() > 8 {
            &self.as_slice()[..10]
        } else {
            self.as_slice()
        };
        write!(f, "OwnedBytes({:?}, len={})", bytes_truncated, self.len())
    }
}

impl Deref for OwnedBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl io::Read for OwnedBytes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read_len = {
            let data = self.as_slice();
            if data.len() >= buf.len() {
                let buf_len = buf.len();
                buf.copy_from_slice(&data[..buf_len]);
                buf.len()
            } else {
                let data_len = data.len();
                buf[..data_len].copy_from_slice(data);
                data_len
            }
        };
        self.advance(read_len);
        Ok(read_len)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let read_len = {
            let data = self.as_slice();
            buf.extend(data);
            data.len()
        };
        self.advance(read_len);
        Ok(read_len)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let read_len = self.read(buf)?;
        if read_len != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ));
        }
        Ok(())
    }
}

impl AsRef<[u8]> for OwnedBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::OwnedBytes;

    #[test]
    fn test_owned_bytes_debug() {
        let short_bytes = OwnedBytes::new(b"abcd".as_ref());
        assert_eq!(
            format!("{:?}", short_bytes),
            "OwnedBytes([97, 98, 99, 100], len=4)"
        );
        let long_bytes = OwnedBytes::new(b"abcdefghijklmnopq".as_ref());
        assert_eq!(
            format!("{:?}", long_bytes),
            "OwnedBytes([97, 98, 99, 100, 101, 102, 103, 104, 105, 106], len=17)"
        );
    }

    #[test]
    fn test_owned_bytes_read() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"abcdefghiklmnopqrstuvwxyz".as_ref());
        {
            let mut buf = [0u8; 5];
            bytes.read_exact(&mut buf[..]).unwrap();
            assert_eq!(&buf, b"abcde");
            assert_eq!(bytes.as_slice(), b"fghiklmnopqrstuvwxyz")
        }
        {
            let mut buf = [0u8; 2];
            bytes.read_exact(&mut buf[..]).unwrap();
            assert_eq!(&buf, b"fg");
            assert_eq!(bytes.as_slice(), b"hiklmnopqrstuvwxyz")
        }
        Ok(())
    }

    #[test]
    fn test_owned_bytes_read_right_at_the_end() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"abcde".as_ref());
        let mut buf = [0u8; 5];
        assert_eq!(bytes.read(&mut buf[..]).unwrap(), 5);
        assert_eq!(&buf, b"abcde");
        assert_eq!(bytes.as_slice(), b"");
        assert_eq!(bytes.read(&mut buf[..]).unwrap(), 0);
        assert_eq!(&buf, b"abcde");
        Ok(())
    }
    #[test]
    fn test_owned_bytes_read_incomplete() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"abcde".as_ref());
        let mut buf = [0u8; 7];
        assert_eq!(bytes.read(&mut buf[..]).unwrap(), 5);
        assert_eq!(&buf[..5], b"abcde");
        assert_eq!(bytes.read(&mut buf[..]).unwrap(), 0);
        Ok(())
    }

    #[test]
    fn test_owned_bytes_read_to_end() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"abcde".as_ref());
        let mut buf = Vec::new();
        bytes.read_to_end(&mut buf)?;
        assert_eq!(buf.as_slice(), b"abcde".as_ref());
        Ok(())
    }

    #[test]
    fn test_owned_bytes_read_u8() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"\xFF".as_ref());
        assert_eq!(bytes.read_u8(), 255);
        assert_eq!(bytes.len(), 0);
        Ok(())
    }

    #[test]
    fn test_owned_bytes_read_u64() -> io::Result<()> {
        let mut bytes = OwnedBytes::new(b"\0\xFF\xFF\xFF\xFF\xFF\xFF\xFF".as_ref());
        assert_eq!(bytes.read_u64(), u64::MAX - 255);
        assert_eq!(bytes.len(), 0);
        Ok(())
    }

    #[test]
    fn test_owned_bytes_split() {
        let bytes = OwnedBytes::new(b"abcdefghi".as_ref());
        let (left, right) = bytes.split(3);
        assert_eq!(left.as_slice(), b"abc");
        assert_eq!(right.as_slice(), b"defghi");
    }

    #[test]
    fn test_owned_bytes_split_boundary() {
        let bytes = OwnedBytes::new(b"abcdefghi".as_ref());
        {
            let (left, right) = bytes.clone().split(0);
            assert_eq!(left.as_slice(), b"");
            assert_eq!(right.as_slice(), b"abcdefghi");
        }
        {
            let (left, right) = bytes.split(9);
            assert_eq!(left.as_slice(), b"abcdefghi");
            assert_eq!(right.as_slice(), b"");
        }
    }
}
