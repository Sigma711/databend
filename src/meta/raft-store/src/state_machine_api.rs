// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Debug;
use std::sync::Arc;

use databend_common_meta_types::sys_data::SysData;
use databend_common_meta_types::SeqV;
use futures::future::BoxFuture;
use map_api::map_api::MapApi;

use crate::state_machine::ExpireKey;
use crate::state_machine::UserKey;

/// Send a key-value change event to subscribers.
pub trait SMEventSender: Debug + Sync + Send {
    fn send(&self, change: (String, Option<SeqV>, Option<SeqV>));

    /// Send a future to the worker to let it run it in serialized order.
    fn send_future(&self, fut: BoxFuture<'static, ()>);
}

impl<T> SMEventSender for Arc<T>
where T: SMEventSender
{
    fn send(&self, change: (String, Option<SeqV>, Option<SeqV>)) {
        self.as_ref().send(change);
    }

    fn send_future(&self, fut: BoxFuture<'static, ()>) {
        self.as_ref().send_future(fut);
    }
}

/// The API a state machine implements.
///
/// The state machine is responsible for managing the application's persistent state,
/// including application kv data and expired key data.
pub trait StateMachineApi: Send + Sync {
    /// The map that stores application data.
    type UserMap: MapApi<UserKey> + 'static;

    /// Returns a reference to the map that stores application data.
    ///
    /// This method provides read-only access to the underlying key-value store
    /// that contains the application's persistent state, including application kv data and expired key data.
    fn user_map(&self) -> &Self::UserMap;

    /// Returns a mutable reference to the map that stores application data.
    ///
    /// This method provides read-write access to the underlying key-value store
    /// that contains the application's persistent state, including application kv data and expired key data.
    /// Changes made through this reference will be persisted according to the state machine's replication
    /// protocol.
    fn user_map_mut(&mut self) -> &mut Self::UserMap;

    /// The map that stores expired key data.
    type ExpireMap: MapApi<ExpireKey> + 'static;

    /// Returns a reference to the map that stores expired key data.
    fn expire_map(&self) -> &Self::ExpireMap;

    /// Returns a mutable reference to the map that stores expired key data.
    fn expire_map_mut(&mut self) -> &mut Self::ExpireMap;

    /// Returns a mutable reference to the system data.
    ///
    /// This method provides read-write access to the system data, which includes
    /// metadata about the state machine and its configuration.
    fn sys_data_mut(&mut self) -> &mut SysData;

    /// Returns an optional reference to the event sender.
    ///
    /// This method returns an event sender that can be used to send state change events to subscribers.
    ///
    /// The implementation could just return `None` if the state machine does not support subscribing.
    fn event_sender(&self) -> Option<&dyn SMEventSender>;
}
