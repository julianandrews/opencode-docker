use nix::unistd::{Gid, Uid};

pub fn uid() -> String {
    Uid::current().to_string()
}

pub fn gid() -> String {
    Gid::current().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_is_valid_number() {
        let uid = uid();
        assert!(!uid.is_empty());
        assert!(uid.parse::<u32>().is_ok(), "uid should be a valid u32");
    }

    #[test]
    fn gid_is_valid_number() {
        let gid = gid();
        assert!(!gid.is_empty());
        assert!(gid.parse::<u32>().is_ok(), "gid should be a valid u32");
    }
}
