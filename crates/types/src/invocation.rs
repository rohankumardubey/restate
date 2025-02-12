// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! This module contains all the core types representing a service invocation.

use crate::errors::{InvocationError, UserErrorCode};
use crate::identifiers::{
    EntryIndex, FullInvocationId, InvocationId, PartitionKey, WithPartitionKey,
};
use crate::GenerationalNodeId;
use bytes::Bytes;
use bytestring::ByteString;
use opentelemetry_api::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceState};
use opentelemetry_api::Context;
use std::fmt;
use std::str::FromStr;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[cfg(feature = "serde")]
use serde_with::{serde_as, FromInto};

// Re-exporting opentelemetry [`TraceId`] to avoid having to import opentelemetry in all crates.
pub use opentelemetry_api::trace::TraceId;

/// Struct representing an invocation to a service. This struct is processed by Restate to execute the invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServiceInvocation {
    pub fid: FullInvocationId,
    pub method_name: ByteString,
    pub argument: Bytes,
    pub source: Source,
    pub response_sink: Option<ServiceInvocationResponseSink>,
    pub span_context: ServiceInvocationSpanContext,
}

impl ServiceInvocation {
    /// Create a new [`ServiceInvocation`].
    ///
    /// This method returns the [`Span`] associated to the created [`ServiceInvocation`].
    /// It is not required to keep this [`Span`] around for the whole lifecycle of the invocation.
    /// On the contrary, it is encouraged to drop it as soon as possible,
    /// to let the exporter commit this span to jaeger/zipkin to visualize intermediate results of the invocation.
    pub fn new(
        fid: FullInvocationId,
        method_name: impl Into<ByteString>,
        argument: impl Into<Bytes>,
        source: Source,
        response_sink: Option<ServiceInvocationResponseSink>,
        related_span: SpanRelation,
    ) -> Self {
        let span_context = ServiceInvocationSpanContext::start(&fid, related_span);
        Self {
            fid,
            method_name: method_name.into(),
            argument: argument.into(),
            source,
            response_sink,
            span_context,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MaybeFullInvocationId {
    Partial(InvocationId),
    Full(FullInvocationId),
}

impl From<MaybeFullInvocationId> for InvocationId {
    fn from(value: MaybeFullInvocationId) -> Self {
        match value {
            MaybeFullInvocationId::Partial(iid) => iid,
            MaybeFullInvocationId::Full(fid) => InvocationId::from(fid),
        }
    }
}

impl From<InvocationId> for MaybeFullInvocationId {
    fn from(value: InvocationId) -> Self {
        MaybeFullInvocationId::Partial(value)
    }
}

impl From<FullInvocationId> for MaybeFullInvocationId {
    fn from(value: FullInvocationId) -> Self {
        MaybeFullInvocationId::Full(value)
    }
}

impl WithPartitionKey for MaybeFullInvocationId {
    fn partition_key(&self) -> PartitionKey {
        match self {
            MaybeFullInvocationId::Partial(iid) => iid.partition_key(),
            MaybeFullInvocationId::Full(fid) => fid.partition_key(),
        }
    }
}

impl fmt::Display for MaybeFullInvocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaybeFullInvocationId::Partial(iid) => fmt::Display::fmt(iid, f),
            MaybeFullInvocationId::Full(fid) => fmt::Display::fmt(fid, f),
        }
    }
}

/// Representing a response for a caller
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InvocationResponse {
    /// Depending on the source of the response, this can be either the full identifier, or the short one.
    pub id: MaybeFullInvocationId,
    pub entry_index: EntryIndex,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ResponseResult {
    Success(Bytes),
    Failure(UserErrorCode, ByteString),
}

impl From<Result<Bytes, InvocationError>> for ResponseResult {
    fn from(value: Result<Bytes, InvocationError>) -> Self {
        match value {
            Ok(v) => ResponseResult::Success(v),
            Err(e) => ResponseResult::from(e),
        }
    }
}

impl From<InvocationError> for ResponseResult {
    fn from(e: InvocationError) -> Self {
        ResponseResult::Failure(e.code().into(), e.message().into())
    }
}

impl From<&InvocationError> for ResponseResult {
    fn from(e: &InvocationError) -> Self {
        ResponseResult::Failure(e.code().into(), e.message().into())
    }
}

/// Definition of the sink where to send the result of a service invocation.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ServiceInvocationResponseSink {
    /// The invocation has been created by a partition processor and is expecting a response.
    PartitionProcessor {
        caller: FullInvocationId,
        entry_index: EntryIndex,
    },
    /// The result needs to be used as input argument of a new invocation
    NewInvocation {
        target: FullInvocationId,
        method: String,
        caller_context: Bytes,
    },
    /// The invocation has been generated by a request received at an ingress, and the client is expecting a response back.
    Ingress(GenerationalNodeId),
}

/// Source of an invocation
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Source {
    Ingress,
    Service(FullInvocationId),
    /// Internal calls for the non-deterministic built-in services
    Internal,
}

/// This struct contains the relevant span information for a [`ServiceInvocation`].
/// It can be used to create related spans, such as child spans,
/// using [`ServiceInvocationSpanContext::as_linked`] or [`ServiceInvocationSpanContext::as_parent`].
#[cfg_attr(feature = "serde", cfg_eval::cfg_eval, serde_as)]
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServiceInvocationSpanContext {
    #[cfg_attr(feature = "serde", serde_as(as = "FromInto<SpanContextDef>"))]
    span_context: SpanContext,
    cause: Option<SpanRelationCause>,
}

impl ServiceInvocationSpanContext {
    pub fn new(span_context: SpanContext, cause: Option<SpanRelationCause>) -> Self {
        Self {
            span_context,
            cause,
        }
    }

    pub fn empty() -> Self {
        Self {
            span_context: SpanContext::empty_context(),
            cause: None,
        }
    }

    /// Create a [`SpanContext`] for this invocation, a [`Span`] which will be created
    /// when the invocation completes.
    ///
    /// This function is **deterministic**.
    pub fn start(
        full_invocation_id: &FullInvocationId,
        related_span: SpanRelation,
    ) -> ServiceInvocationSpanContext {
        if !related_span.is_sampled() {
            // don't waste any time or storage space on unsampled traces
            // sampling based on parent is default otel behaviour; we do the same for the
            // non-parent background invoke relationship
            return ServiceInvocationSpanContext::empty();
        }

        let (cause, new_span_context) = match &related_span {
            SpanRelation::Linked(linked_span_context) => {
                // use part of the invocation id as the span id of the new trace root
                let span_id: SpanId = full_invocation_id.invocation_uuid.into();

                // use its reverse as the span id of the background_invoke 'pointer' span in the previous trace
                // as we cannot use the same span id for both spans
                let mut pointer_span_id = span_id.to_bytes();
                pointer_span_id.reverse();

                // create a span context with a new trace that will be used for any actions as part of the background invocation
                // a span will be emitted using these details when its finished (so we know how long the invocation took)
                let new_span_context = SpanContext::new(
                    // use invocation id as the new trace id; this allows you to follow cause -> new trace in jaeger
                    // trace ids are 128 bits and 'worldwide unique'
                    full_invocation_id.invocation_uuid.into(),
                    // use part of the invocation id as the new span id; this is 64 bits and best-effort 'globally unique'
                    span_id,
                    // use sampling decision of the causing trace; this is NOT default otel behaviour but
                    // is useful for users
                    linked_span_context.trace_flags(),
                    // this would never be set to true for a span created in this binary
                    false,
                    TraceState::default(),
                );
                let cause = SpanRelationCause::Linked(
                    linked_span_context.trace_id(),
                    SpanId::from_bytes(pointer_span_id),
                );
                (Some(cause), new_span_context)
            }
            SpanRelation::Parent(parent_span_context) => {
                // create a span context as part of the existing trace, which will be used for any actions
                // of the invocation. a span will be emitted with these details when its finished
                let new_span_context = SpanContext::new(
                    // use parent trace id
                    parent_span_context.trace_id(),
                    // use part of the invocation id as the new span id
                    full_invocation_id.invocation_uuid.into(),
                    // use sampling decision of parent trace; this is default otel behaviour
                    parent_span_context.trace_flags(),
                    false,
                    parent_span_context.trace_state().clone(),
                );
                let cause = SpanRelationCause::Parent(parent_span_context.span_id());
                (Some(cause), new_span_context)
            }
            SpanRelation::None => {
                // we would only expect this in tests as there should always be either another invocation
                // or an ingress task leading to the invocation

                // create a span context with a new trace
                let new_span_context = SpanContext::new(
                    // use invocation id as the new trace id and span id
                    full_invocation_id.invocation_uuid.into(),
                    full_invocation_id.invocation_uuid.into(),
                    // we don't have the means to actually sample here; just hardcode a sampled trace
                    // as this should only happen in tests anyway
                    TraceFlags::SAMPLED,
                    false,
                    TraceState::default(),
                );
                (None, new_span_context)
            }
        };

        ServiceInvocationSpanContext {
            span_context: new_span_context,
            cause,
        }
    }

    pub fn causing_span_relation(&self) -> SpanRelation {
        match self.cause {
            None => SpanRelation::None,
            Some(SpanRelationCause::Parent(span_id)) => {
                SpanRelation::Parent(SpanContext::new(
                    // in invoke case, trace id of cause matches that of child
                    self.span_context.trace_id(),
                    // use stored span id
                    span_id,
                    // use child trace flags as the cause trace flags; when this is set as parent
                    // the flags will be set on the child
                    self.span_context.trace_flags(),
                    // this will be ignored; is_remote is not propagated
                    false,
                    // use child trace state as the cause trace state; when this is set as parent
                    // the state will be set on the child
                    self.span_context.trace_state().clone(),
                ))
            }
            Some(SpanRelationCause::Linked(trace_id, span_id)) => {
                SpanRelation::Linked(SpanContext::new(
                    // use stored trace id
                    trace_id,
                    // use stored span id
                    span_id,
                    // this will be ignored; trace flags are not propagated to links
                    self.span_context.trace_flags(),
                    // this will be ignored; is_remote is not propagated
                    false,
                    // this will be ignored; trace state is not propagated to links
                    TraceState::default(),
                ))
            }
        }
    }

    pub fn span_context(&self) -> &SpanContext {
        &self.span_context
    }

    pub fn span_cause(&self) -> Option<&SpanRelationCause> {
        self.cause.as_ref()
    }

    pub fn as_linked(&self) -> SpanRelation {
        SpanRelation::Linked(self.span_context.clone())
    }

    pub fn as_parent(&self) -> SpanRelation {
        SpanRelation::Parent(self.span_context.clone())
    }

    pub fn is_sampled(&self) -> bool {
        self.span_context.trace_flags().is_sampled()
    }

    pub fn trace_id(&self) -> TraceId {
        self.span_context.trace_id()
    }
}

impl Default for ServiceInvocationSpanContext {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<ServiceInvocationSpanContext> for SpanContext {
    fn from(value: ServiceInvocationSpanContext) -> Self {
        value.span_context
    }
}

/// Span relation cause, used to propagate tracing contexts.
#[cfg_attr(feature = "serde", cfg_eval::cfg_eval, serde_as)]
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SpanRelationCause {
    Parent(#[cfg_attr(feature = "serde", serde_as(as = "FromInto<SpanIdDef>"))] SpanId),
    Linked(
        #[cfg_attr(feature = "serde", serde_as(as = "FromInto<TraceIdDef>"))] TraceId,
        #[cfg_attr(feature = "serde", serde_as(as = "FromInto<SpanIdDef>"))] SpanId,
    ),
}

#[derive(Default)]
pub enum SpanRelation {
    #[default]
    None,
    Parent(SpanContext),
    Linked(SpanContext),
}

impl SpanRelation {
    /// Attach this [`SpanRelation`] to the given [`Span`]
    pub fn attach_to_span(self, span: &Span) {
        match self {
            SpanRelation::Parent(span_context) => {
                span.set_parent(Context::new().with_remote_span_context(span_context))
            }
            SpanRelation::Linked(span_context) => span.add_link(span_context),
            SpanRelation::None => (),
        };
    }

    fn is_sampled(&self) -> bool {
        match self {
            SpanRelation::None => false,
            SpanRelation::Parent(span_context) => span_context.is_sampled(),
            SpanRelation::Linked(span_context) => span_context.is_sampled(),
        }
    }
}

/// Message to terminate an invocation.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InvocationTermination {
    pub maybe_fid: MaybeFullInvocationId,
    pub flavor: TerminationFlavor,
}

impl InvocationTermination {
    pub fn kill(maybe_fid: impl Into<MaybeFullInvocationId>) -> Self {
        Self {
            maybe_fid: maybe_fid.into(),
            flavor: TerminationFlavor::Kill,
        }
    }

    pub fn cancel(maybe_fid: impl Into<MaybeFullInvocationId>) -> Self {
        Self {
            maybe_fid: maybe_fid.into(),
            flavor: TerminationFlavor::Cancel,
        }
    }
}

/// Flavor of the termination. Can be kill (hard stop) or graceful cancel.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TerminationFlavor {
    /// hard termination, no clean up
    Kill,
    /// graceful termination allowing the invocation to clean up
    Cancel,
}

// A hack to allow spancontext to be serialized.
// Details in https://github.com/open-telemetry/opentelemetry-rust/issues/576#issuecomment-1253396100
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct SpanContextDef {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    trace_flags: u8,
    is_remote: bool,
    trace_state: String,
}

// Provide a conversion to construct the remote type.
impl From<SpanContextDef> for SpanContext {
    fn from(def: SpanContextDef) -> Self {
        SpanContext::new(
            TraceId::from_bytes(def.trace_id),
            SpanId::from_bytes(def.span_id),
            TraceFlags::new(def.trace_flags),
            def.is_remote,
            TraceState::from_str(&def.trace_state).unwrap(),
        )
    }
}

impl From<SpanContext> for SpanContextDef {
    fn from(ctx: SpanContext) -> Self {
        Self {
            trace_id: ctx.trace_id().to_bytes(),
            span_id: ctx.span_id().to_bytes(),
            trace_flags: ctx.trace_flags().to_u8(),
            is_remote: ctx.is_remote(),
            trace_state: ctx.trace_state().header(),
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct SpanIdDef([u8; 8]);

impl From<SpanIdDef> for SpanId {
    fn from(def: SpanIdDef) -> Self {
        SpanId::from_bytes(def.0)
    }
}

impl From<SpanId> for SpanIdDef {
    fn from(def: SpanId) -> Self {
        SpanIdDef(def.to_bytes())
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct TraceIdDef([u8; 16]);

impl From<TraceIdDef> for TraceId {
    fn from(def: TraceIdDef) -> Self {
        TraceId::from_bytes(def.0)
    }
}

impl From<TraceId> for TraceIdDef {
    fn from(def: TraceId) -> Self {
        TraceIdDef(def.to_bytes())
    }
}

#[cfg(any(test, feature = "mocks"))]
mod mocks {
    use super::*;

    impl ServiceInvocation {
        pub fn mock() -> Self {
            Self {
                fid: FullInvocationId::mock_random(),
                method_name: "mock".into(),
                argument: Default::default(),
                source: Source::Service(FullInvocationId::mock_random()),
                response_sink: None,
                span_context: Default::default(),
            }
        }
    }
}
