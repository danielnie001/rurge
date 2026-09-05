//! The REJECT family: never connects; the listener turns the error into the
//! protocol-appropriate response (M3 design §8).

use crate::outbound::{Outbound, OutboundError, RejectKind};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};

pub struct Reject {
    kind: RejectKind,
}

impl Reject {
    pub fn new(kind: RejectKind) -> Reject {
        Reject { kind }
    }

    pub fn kind(&self) -> RejectKind {
        self.kind
    }
}

impl Outbound for Reject {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn connect_tcp<'a>(
        &'a self,
        _target: &'a Target,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(std::future::ready(Err(OutboundError::Reject(self.kind))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::policy::Builtin;

    #[tokio::test]
    async fn every_kind_rejects_immediately_with_its_name() {
        for (builtin, kind, name) in [
            (Builtin::Reject, RejectKind::Reject, "REJECT"),
            (Builtin::RejectDrop, RejectKind::Drop, "REJECT-DROP"),
            (Builtin::RejectNoDrop, RejectKind::NoDrop, "REJECT-NO-DROP"),
            (
                Builtin::RejectTinyGif,
                RejectKind::TinyGif,
                "REJECT-TINYGIF",
            ),
        ] {
            assert_eq!(RejectKind::from_builtin(builtin), Some(kind));
            let r = Reject::new(kind);
            assert_eq!(r.name(), name);
            assert_eq!(r.kind(), kind);
            let err = r
                .connect_tcp(
                    &Target::new(HostName::parse("a.test"), 443),
                    &ConnectOpts::default(),
                )
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(matches!(err, OutboundError::Reject(k) if k == kind));
        }
        assert_eq!(RejectKind::from_builtin(Builtin::Direct), None);
        assert!(RejectKind::Reject.escalates() && RejectKind::TinyGif.escalates());
        assert!(!RejectKind::Drop.escalates() && !RejectKind::NoDrop.escalates());
        assert_eq!(
            OutboundError::Unsupported("ss".into()).to_string(),
            "policy protocol not implemented: ss"
        );
    }
}
