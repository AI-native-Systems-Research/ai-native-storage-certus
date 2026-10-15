//! Repair test for ZYR-CREATE-NODE-APPLIES-HEADERS: a configured header value
//! must reach peers byte-for-byte, even when it contains `%`.

use super::*;
use interfaces::GossipConfig;
use std::time::{Duration, Instant};

#[test]
fn header_value_with_percent_is_announced_verbatim() {
    // Node A carries a header whose value contains `%%`; node B discovers A
    // over loopback gossip and reports A's headers in the Enter event.
    let mut config_a = NodeConfig::default();
    config_a.name = Some("repair-hdr-a".into());
    config_a.endpoint = Some("tcp://127.0.0.1:19987".into());
    config_a.gossip = Some(GossipConfig::bind("tcp://127.0.0.1:19986"));
    config_a.headers.insert("X-Load".into(), "100%%".into());

    let mut config_b = NodeConfig::default();
    config_b.name = Some("repair-hdr-b".into());
    config_b.endpoint = Some("tcp://127.0.0.1:19988".into());
    config_b.gossip = Some(GossipConfig::connect("tcp://127.0.0.1:19986"));

    let mut node_a = ZyreNode::new(config_a).expect("create node A");
    let mut node_b = ZyreNode::new(config_b).expect("create node B");
    node_a.start().expect("start A");
    node_b.start().expect("start B");

    let mut announced = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match node_b.try_recv() {
            Ok(Some(ZyreEvent::Enter { name, headers, .. })) if name == "repair-hdr-a" => {
                announced = Some(headers);
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("recv error: {e}"),
        }
    }

    let headers = announced.expect("node B should discover node A via gossip");
    assert_eq!(
        headers.get("X-Load").map(String::as_str),
        Some("100%%"),
        "header value must be announced exactly as configured"
    );

    node_b.stop();
    node_a.stop();
}
