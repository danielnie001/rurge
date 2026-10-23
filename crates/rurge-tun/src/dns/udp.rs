//! The UDP side of the responder (phase 3 M3 design 11.4): the queue the receive
//! task offers the DNS queries of the apps to, the task that takes them off it,
//! and the task of each query.
//!
//! The receive task never waits, so a query goes into a bounded queue
//! ([`offer`]) and is dropped and counted when the queue is full. One task
//! takes the queries off the queue and starts a task for each on the TUN's
//! tracker, which asks the responder and writes the answer: a fake answer is
//! made on the spot, a forwarded one when the upstream has answered. Every task
//! ends at the stop, wherever it is. The answer is handed to the writer as the
//! TUN's other local answers are, without waiting for room; a task that waited
//! would wait for as long as the device takes nothing, and the answers made on
//! the spot take no place of the responder's limit on forwards, so the tasks
//! would pile up behind a device that is stuck.

use super::Ask;
use crate::context::Context;
use crate::fragment::{Ids, ip_packets};
use crate::packet::{Reason, udp_datagram};
use crate::stats::Stats;
use smoltcp::wire::IpProtocol;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// A query on its way to the responder: the bytes of the DNS message as the app
/// sent them, and who sent it to whom.
pub(crate) struct Query {
    pub(crate) payload: Vec<u8>,
    pub(crate) ask: Ask,
}

/// Queues the query in `payload`, which the app at `ask.src` sent to
/// `ask.server`, for the responder, without waiting: if the queue is full the
/// query is dropped and counted `queue_full`. A queue that is closed means that
/// the TUN has stopped, and the query is dropped without a count.
pub(crate) fn offer(queue: &mpsc::Sender<Query>, stats: &Stats, payload: &[u8], ask: Ask) {
    let query = Query {
        payload: payload.to_vec(),
        ask,
    };
    match queue.try_send(query) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            Stats::drop(
                stats,
                Reason::QueueFull,
                Some(ask.src.ip()),
                Some(ask.server.ip()),
            );
        }
        // the queue's task has ended, and so has the TUN
        Err(TrySendError::Closed(_)) => {}
    }
}

/// The task that takes the queries off the queue and starts the task of each. It
/// is one of the TUN's own tasks and ends at the stop; the queue does not close
/// by itself, for the context holds its sender.
pub(crate) async fn run(ctx: Arc<Context>, mut queries: mpsc::Receiver<Query>) {
    loop {
        let query = tokio::select! {
            biased;
            _ = ctx.life.stopped() => return,
            query = queries.recv() => query,
        };
        let Some(query) = query else {
            return;
        };
        ctx.tasks.spawn(respond(ctx.clone(), query));
    }
}

/// The task of one query: the responder's answer, written for the app. It ends
/// at the stop, so that a query the upstream holds does not keep the TUN from
/// ending. The responder says what it decided in a line of its own (at debug
/// level).
async fn respond(ctx: Arc<Context>, query: Query) {
    let Query { payload, ask } = query;
    let reply = tokio::select! {
        biased;
        _ = ctx.life.stopped() => return,
        reply = ctx.responder.answer(&payload, &ask) => reply,
    };
    if let Some(reply) = reply {
        write(&ctx, &ask, &reply);
    }
}

/// The IP packets that carry `reply` from the endpoint the query was sent to,
/// to the endpoint it came from: one packet, or the fragments of one if it does
/// not fit the `mtu`. `None` if the reply is too long to be a datagram, which
/// the responder's replies over UDP, fitted into 1,232 bytes, never are.
pub(crate) fn reply_packets(
    ask: &Ask,
    reply: &[u8],
    mtu: usize,
    ids: &Ids,
) -> Option<Vec<Vec<u8>>> {
    let datagram = udp_datagram(ask.server, ask.src, reply)?;
    Some(ip_packets(
        ask.server.ip(),
        ask.src.ip(),
        IpProtocol::Udp,
        &datagram,
        mtu,
        ids,
    ))
}

/// Hands the writer the datagram that carries `reply` (see [`reply_packets`]),
/// without waiting for room: if the writer's queue is full the answer is
/// dropped and counted `queue_full` (the app asks again), and if it is closed
/// the TUN has ended.
fn write(ctx: &Context, ask: &Ask, reply: &[u8]) {
    let Some(packets) = reply_packets(ask, reply, ctx.mtu, &ctx.ids) else {
        return;
    };
    match ctx.writer.try_send(packets) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            Stats::drop(
                &ctx.stats,
                Reason::QueueFull,
                Some(ask.server.ip()),
                Some(ask.src.ip()),
            );
        }
        // the writer has ended, and so has the TUN
        Err(TrySendError::Closed(_)) => {}
    }
}
