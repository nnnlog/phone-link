use super::*;

#[test]
fn filters() {
    assert!(usable("192.168.1.2".parse().unwrap()));
    assert!(usable("fe80::1".parse().unwrap()));
    assert!(!usable("127.0.0.1".parse().unwrap()));
    assert!(!usable("::1".parse().unwrap()));
    assert!(!usable("fec0::1".parse().unwrap()));
    assert!(!usable("::ffff:1.2.3.4".parse().unwrap()));

    assert!(host_addresses().is_ok());
}
