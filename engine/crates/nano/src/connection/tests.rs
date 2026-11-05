use super::*;

#[tokio::test]
async fn the_turn_host_is_reached_only_over_ipv4() {
    let v4: SocketAddr = "127.0.0.1:3478".parse().unwrap();
    let v6: SocketAddr = "[::1]:3478".parse().unwrap();

    assert_eq!(resolve("127.0.0.1", 3478, turn_address_usable).await, Some(v4));
    assert_eq!(resolve("::1", 3478, turn_address_usable).await, None);
    assert_eq!(resolve("::1", 3478, |_| true).await, Some(v6), "STUN hosts keep either family");
}
