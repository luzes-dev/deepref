//! A permit that records every request it is asked for, shared by the adapter tests.

use std::sync::Mutex;

use deepref_application::{
    OutboundRequest, OutboundService, PermitError, ProviderFuture, ProviderPermit,
};
use deepref_domain::ProviderKind;

#[derive(Default)]
pub(crate) struct RecordingPermit(Mutex<Vec<OutboundRequest>>);

impl RecordingPermit {
    /// The metadata providers asked for, in order. A Europe PMC request is not one.
    pub(crate) fn taken(&self) -> Vec<ProviderKind> {
        self.requests()
            .iter()
            .filter_map(|request| request.service.provider())
            .collect()
    }

    /// Every request asked for, in order.
    pub(crate) fn requests(&self) -> Vec<OutboundRequest> {
        self.0.lock().expect("permit log").clone()
    }

    /// How many requests went to `service`.
    pub(crate) fn count(&self, service: OutboundService) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.service == service)
            .count()
    }
}

impl ProviderPermit for RecordingPermit {
    fn acquire<'a>(&'a self, request: OutboundRequest) -> ProviderFuture<'a, (), PermitError> {
        Box::pin(async move {
            self.0.lock().expect("permit log").push(request);
            Ok(())
        })
    }
}
