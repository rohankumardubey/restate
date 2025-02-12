// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use crate::partition::state_machine::commands::AckResponse;
use crate::partition::TimerValue;
use bytes::Bytes;
use bytestring::ByteString;
use restate_invoker_api::InvokeInputJournal;
use restate_storage_api::outbox_table::OutboxMessage;
use restate_storage_api::timer_table::TimerKey;
use restate_types::identifiers::{EntryIndex, FullInvocationId, InvocationUuid, ServiceId};
use restate_types::invocation::{ServiceInvocationResponseSink, ServiceInvocationSpanContext};
use restate_types::journal::Completion;
use restate_types::message::MessageIndex;

#[derive(Debug)]
pub enum Action {
    Invoke {
        full_invocation_id: FullInvocationId,
        invoke_input_journal: InvokeInputJournal,
    },
    InvokeBuiltInService {
        full_invocation_id: FullInvocationId,
        method: ByteString,
        span_context: ServiceInvocationSpanContext,
        response_sink: Option<ServiceInvocationResponseSink>,
        argument: Bytes,
    },
    NotifyVirtualJournalCompletion {
        target_service: ServiceId,
        method_name: String,
        invocation_uuid: InvocationUuid,
        completion: Completion,
    },
    NotifyVirtualJournalKill {
        target_service: ServiceId,
        method_name: String,
        invocation_uuid: InvocationUuid,
    },
    NewOutboxMessage {
        seq_number: MessageIndex,
        message: OutboxMessage,
    },
    RegisterTimer {
        timer_value: TimerValue,
    },
    DeleteTimer {
        timer_key: TimerKey,
    },
    AckStoredEntry {
        full_invocation_id: FullInvocationId,
        entry_index: EntryIndex,
    },
    ForwardCompletion {
        full_invocation_id: FullInvocationId,
        completion: Completion,
    },
    SendAckResponse(AckResponse),
    AbortInvocation(FullInvocationId),
}
