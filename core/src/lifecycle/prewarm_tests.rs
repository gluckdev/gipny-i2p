use super::*;
#[test]
fn accepting_a_prepared_outgoing_destination_requires_both_directions_and_live_readiness() {
    for incoming in [false, true] {
        for outgoing in [false, true] {
            for ready in [false, true] {
                assert_eq!(
                    destination_ready((incoming, outgoing), ready),
                    incoming && outgoing && ready
                );
            }
        }
    }
}
