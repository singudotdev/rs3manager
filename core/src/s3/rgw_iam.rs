//! A fix for radosgw 19.2's IAM answers the AWS SDK can't read: `ListUserPolicies`, `GetUserPolicy` and
//! `ListAttachedUserPolicies` put `<ResponseMetadata>` before the `...Result` element, and the SDK's query-protocol
//! parser expects the result first (`invalid result, expected ListUserPoliciesResult got ResponseMetadata`). The AWS
//! CLI's parser doesn't care, so the gateway's bug goes unnoticed there. This interceptor drops the metadata (a request
//! id) from every IAM answer before the SDK parses it.
//!
//! The answer's body is only read after the last hook that may change it, so the body is wrapped: it collects what the
//! gateway sends and hands the SDK the fixed document once complete.

use aws_sdk_iam::config::interceptors::BeforeDeserializationInterceptorContextMut;
use aws_sdk_iam::config::{ConfigBag, Intercept, RuntimeComponents};
use aws_sdk_iam::error::BoxError;
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use http_body::{Body, Frame};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

/// The interceptor, for the IAM client's config.
#[derive(Debug)]
pub struct DropResponseMetadata;

impl Intercept for DropResponseMetadata {
    fn name(&self) -> &'static str {
        "DropResponseMetadata"
    }

    fn modify_before_deserialization(
        &self,
        context: &mut BeforeDeserializationInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        _: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let response = context.response_mut();
        let body = response.take_body();
        *response.body_mut() = SdkBody::from_body_1_x(Fixed { inner: body, read: Vec::new(), done: false });
        Ok(())
    }
}

/// A body that collects `inner` and yields it once, without its `<ResponseMetadata>` element.
struct Fixed {
    /// What the gateway sends.
    inner: SdkBody,
    /// What it has sent so far.
    read: Vec<u8>,
    /// Whether the fixed document was handed over.
    done: bool,
}

impl Body for Fixed {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        if self.done {
            return Poll::Ready(None);
        }
        loop {
            match ready!(Pin::new(&mut self.inner).poll_frame(cx)) {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        self.read.extend_from_slice(data);
                    }
                }
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => {
                    self.done = true;
                    let fixed = without_metadata(&self.read);
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(fixed)))));
                }
            }
        }
    }
}

/// `xml` without its `<ResponseMetadata>...</ResponseMetadata>` element, if it has one.
fn without_metadata(xml: &[u8]) -> Vec<u8> {
    const OPEN: &[u8] = b"<ResponseMetadata>";
    const CLOSE: &[u8] = b"</ResponseMetadata>";
    let find =
        |needle: &[u8], from: usize| xml[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from);
    match find(OPEN, 0).and_then(|start| Some((start, find(CLOSE, start)? + CLOSE.len()))) {
        Some((start, end)) => [&xml[..start], &xml[end..]].concat(),
        None => xml.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metadata_goes_wherever_it_is() {
        let rgw = b"<R><ResponseMetadata><RequestId>tx1</RequestId></ResponseMetadata><Result><M>p</M></Result></R>";
        assert_eq!(without_metadata(rgw), b"<R><Result><M>p</M></Result></R>");
        let aws = b"<R><Result/><ResponseMetadata><RequestId>x</RequestId></ResponseMetadata></R>";
        assert_eq!(without_metadata(aws), b"<R><Result/></R>");
        assert_eq!(without_metadata(b"<R><Result/></R>"), b"<R><Result/></R>");
    }
}
