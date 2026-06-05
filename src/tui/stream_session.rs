use std::collections::HashSet;

use crate::api::{DeliveryListItem, DeliveryStreamFrame, StreamCursor};

/// Owns route-delivery stream checkpoint and replay suppression state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DeliveryStreamSession {
    resume_cursor: Option<StreamCursor>,
    seen_cursors: HashSet<StreamCursor>,
}

impl DeliveryStreamSession {
    /// Returns the cursor that should be sent as the next stream `after` value.
    pub(super) fn resume_cursor(&self) -> Option<&StreamCursor> {
        self.resume_cursor.as_ref()
    }

    /// Seeds stream replay protection from an initial list page.
    pub(super) fn track_page_cursors(&mut self, deliveries: &[DeliveryListItem]) {
        let latest_page_cursor = deliveries.iter().find_map(|item| item.cursor().cloned());

        for cursor in deliveries.iter().filter_map(DeliveryListItem::cursor) {
            self.seen_cursors.insert(cursor.clone());
        }

        if self.resume_cursor.is_none() {
            self.resume_cursor = latest_page_cursor;
        }
    }

    /// Applies stream frames and returns delivery rows that should be inserted.
    pub(super) fn accept_frames(
        &mut self,
        frames: Vec<DeliveryStreamFrame>,
    ) -> Vec<DeliveryListItem> {
        let mut deliveries = Vec::new();

        for frame in frames {
            match frame {
                DeliveryStreamFrame::Connected => {}
                DeliveryStreamFrame::Cursor(cursor) => self.resume_cursor = Some(cursor),
                DeliveryStreamFrame::Delivery { cursor, item } => {
                    let cursor = cursor.or_else(|| item.cursor().cloned());

                    if let Some(cursor) = cursor {
                        if !self.seen_cursors.insert(cursor.clone()) {
                            continue;
                        }

                        self.resume_cursor = Some(cursor);
                    }

                    deliveries.push(item);
                }
            }
        }

        deliveries
    }
}

fn frame_cursor(frame: &DeliveryStreamFrame) -> Option<StreamCursor> {
    match frame {
        DeliveryStreamFrame::Connected => None,
        DeliveryStreamFrame::Cursor(cursor) => Some(cursor.clone()),
        DeliveryStreamFrame::Delivery { cursor, item } => {
            cursor.clone().or_else(|| item.cursor().cloned())
        }
    }
}

/// Tracks reconnect checkpoints for the long-lived delivery stream worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryStreamReconnect {
    resume_cursor: Option<StreamCursor>,
    received_frame: bool,
}

impl DeliveryStreamReconnect {
    pub(super) fn new(initial_after: Option<StreamCursor>) -> Self {
        Self {
            resume_cursor: initial_after,
            received_frame: false,
        }
    }

    pub(super) fn begin_attempt(&mut self) -> Option<StreamCursor> {
        self.received_frame = false;
        self.resume_cursor.clone()
    }

    pub(super) fn accept_frame(&mut self, frame: &DeliveryStreamFrame) {
        if let Some(cursor) = frame_cursor(frame) {
            self.resume_cursor = Some(cursor);
        }

        self.received_frame = true;
    }

    pub(super) fn received_frame(&self) -> bool {
        self.received_frame
    }
}
