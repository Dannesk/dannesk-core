

use zeroize::Zeroize;


#[cfg(all(unix, not(debug_assertions)))]
pub fn harden_process() {
    unsafe {
        let no_core = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        let _ = libc::setrlimit(libc::RLIMIT_CORE, &no_core);

        #[cfg(target_os = "linux")]
        {
            let _ = libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0, 0, 0);
        }
    }
}

#[cfg(any(not(unix), debug_assertions))]
pub fn harden_process() {}


pub struct SecureBytes {
    buf: Vec<u8>,
    guard: Option<region::LockGuard>,
}

impl SecureBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        let guard = if bytes.is_empty() {
            None
        } else {
            region::lock(bytes.as_ptr(), bytes.len()).ok()
        };
        SecureBytes { buf: bytes, guard }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

}

impl Drop for SecureBytes {
    fn drop(&mut self) {
        self.buf.zeroize();
        drop(self.guard.take());
    }
}


const INPUT_CAP: usize = 512;


fn lock_capacity(buf: &[u8], cap: usize) -> Option<region::LockGuard> {
    if cap == 0 {
        None
    } else {
        region::lock(buf.as_ptr(), cap).ok()
    }
}


pub struct SecureString {
    buf: Vec<u8>,
    guard: Option<region::LockGuard>,
}

impl SecureString {

    pub fn new(s: String) -> Self {
        let buf = s.into_bytes();
        let guard = lock_capacity(&buf, buf.capacity());
        SecureString { buf, guard }
    }

    pub fn input() -> Self {
        let buf = Vec::with_capacity(INPUT_CAP);
        let guard = lock_capacity(&buf, buf.capacity());
        SecureString { buf, guard }
    }

  
    fn empty() -> Self {
        SecureString { buf: Vec::new(), guard: None }
    }

 
    pub fn take(&mut self) -> SecureString {
        std::mem::replace(self, SecureString::empty())
    }

 
    pub fn take_trimmed(&mut self) -> SecureString {
        let (start, len) = {
            let s = self.as_str();
            let t = s.trim();
            (t.as_ptr() as usize - s.as_ptr() as usize, t.len())
        };
        let old_len = self.buf.len();
        self.buf.copy_within(start..start + len, 0);
        for b in &mut self.buf[len..old_len] {
            *b = 0;
        }
        self.buf.truncate(len);
        self.take()
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf).unwrap_or("")
    }

   
    pub fn trimmed_char_len(&self) -> usize {
        self.as_str().trim().chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn char_len(&self) -> usize {
        self.as_str().chars().count()
    }

   
    fn ensure(&mut self, extra: usize) {
        if self.buf.len() + extra <= self.buf.capacity() {
            return;
        }
        let new_cap = (self.buf.len() + extra).max(self.buf.capacity() * 2).max(INPUT_CAP);
        let mut new_buf = Vec::with_capacity(new_cap);
        new_buf.extend_from_slice(&self.buf);
        let new_guard = lock_capacity(&new_buf, new_buf.capacity());
        self.buf.zeroize();
        self.buf = new_buf; 
        self.guard = new_guard;
    }

    fn byte_index(&self, char_idx: usize) -> usize {
        self.as_str()
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.buf.len())
    }

    fn insert_bytes(&mut self, at: usize, bytes: &[u8]) {
        let n = bytes.len();
        if n == 0 {
            return;
        }
        self.ensure(n);
        let old_len = self.buf.len();
        self.buf.resize(old_len + n, 0); 
        self.buf.copy_within(at..old_len, at + n);
        self.buf[at..at + n].copy_from_slice(bytes);
    }

    pub fn insert(&mut self, char_idx: usize, c: char) {
        let at = self.byte_index(char_idx);
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp);
        self.insert_bytes(at, s.as_bytes());
        tmp.zeroize();
    }

    pub fn insert_str(&mut self, char_idx: usize, s: &str) {
        let at = self.byte_index(char_idx);
        self.insert_bytes(at, s.as_bytes());
    }

    pub fn remove(&mut self, char_idx: usize) {
        let start = self.byte_index(char_idx);
        let n = match self.as_str()[start..].chars().next() {
            Some(c) => c.len_utf8(),
            None => return,
        };
        let old_len = self.buf.len();
        self.buf.copy_within(start + n.., start);
        let new_len = old_len - n;
        for b in &mut self.buf[new_len..old_len] {
            *b = 0;
        }
        self.buf.truncate(new_len);
    }

    pub fn clear(&mut self) {
        self.buf.zeroize();
        self.buf.clear();
    }
}

impl Drop for SecureString {
    fn drop(&mut self) {
        self.buf.zeroize();
        drop(self.guard.take());
    }
}

impl std::fmt::Debug for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecureString(<redacted; {} chars>)", self.char_len())
    }
}

impl std::fmt::Debug for SecureBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecureBytes(<redacted; {} bytes>)", self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::SecureString;

    #[test]
    fn take_trimmed_trims_the_ends_and_nothing_else() {
        let take = |s: &str| SecureString::new(s.to_string()).take_trimmed().as_str().to_string();
        assert_eq!(take("  1234---55553 \n"), "1234---55553");
        assert_eq!(take("1234   55553"), "1234   55553");
        assert_eq!(take("correct horse battery "), "correct horse battery");
        assert_eq!(take("   "), "");
        assert_eq!(take(""), "");
    }

    #[test]
    fn take_trimmed_empties_the_field_and_leaves_no_tail() {
        let mut field = SecureString::new("  secret  ".to_string());
        let taken = field.take_trimmed();
        assert!(field.is_empty());
        assert_eq!(taken.as_str(), "secret");
        assert_eq!(taken.buf.len(), "secret".len());
    }

    #[test]
    fn trimmed_char_len_ignores_only_the_ends() {
        assert_eq!(SecureString::new("  12 345  ".to_string()).trimmed_char_len(), 6);
        assert_eq!(SecureString::new("      ".to_string()).trimmed_char_len(), 0);
    }
}
