macro_rules! impl_act_state {
    ($type: tt) => {
        impl $type {
            pub(crate) fn set_state(&self, state: &TaskState) {
                let state = state.clone();
                let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
                let time_millis = time.timestamp_millis();
                if state == TaskState::Running || state == TaskState::WaitingEvent {
                    *self.start_time.write().unwrap() = time_millis;
                } else {
                    if state != TaskState::None && state != TaskState::Pending {
                        *self.end_time.write().unwrap() = time_millis;
                    }
                }

                *self.state.write().unwrap() = state;
            }

            #[allow(unused)]
            pub(crate) fn pending(&self) -> bool {
                let state = self.state();
                state == TaskState::Pending
            }

            #[allow(unused)]
            pub(crate) fn waiting(&self) -> bool {
                let state = self.state();
                state == TaskState::WaitingEvent
            }

            #[allow(unused)]
            pub(crate) fn failed(&self) -> bool {
                let state = self.state();
                match state {
                    TaskState::Abort(..) | TaskState::Fail(..) => true,
                    _ => false,
                }
            }

            #[allow(unused)]
            pub(crate) fn finished(&self) -> bool {
                let state = self.state();
                match state {
                    TaskState::None
                    | TaskState::Running
                    | TaskState::Pending
                    | TaskState::WaitingEvent => false,
                    _ => true,
                }
            }

            pub fn state(&self) -> TaskState {
                let state = &*self.state.read().unwrap();
                state.clone()
            }
        }
    };
}

macro_rules! impl_act_time {
    ($type: tt) => {
        impl $type {
            pub fn start_time(&self) -> i64 {
                *self.start_time.read().unwrap()
            }
            pub fn end_time(&self) -> i64 {
                *self.end_time.read().unwrap()
            }

            pub(crate) fn set_start_time(&self, time: i64) {
                *self.start_time.write().unwrap() = time;
            }
            pub(crate) fn set_end_time(&self, time: i64) {
                *self.end_time.write().unwrap() = time;
            }
        }
    };
}

macro_rules! impl_act_id {
    ($type: tt) => {
        impl $type {
            pub fn id(&self) -> String {
                self.id.clone()
            }
            pub fn set_id(&mut self, id: &str) {
                self.id = id.to_string();
            }
        }
    };
}

macro_rules! impl_act_acts {
    ($type: tt) => {
        impl $type {
            pub fn ord(&self) -> usize {
                *self.act_ord.read().unwrap()
            }
            pub(in crate::sch) fn set_ord(&self, ord: usize) {
                *self.act_ord.write().unwrap() = ord;
            }

            pub fn acts(&self) -> Vec<Act> {
                self.acts.read().unwrap().clone()
            }

            pub(in crate::sch) fn push_act(&self, act: &Act) {
                debug!("push_act: {}", act.id);
                self.acts.write().unwrap().push(act.clone());
            }

            pub fn candidates(&self) -> Vec<Act> {
                self.act_candidates.read().unwrap().clone()
            }

            pub(in crate::sch) fn set_candidates(&self, acts: &Vec<Act>) {
                *self.act_candidates.write().unwrap() = acts.clone();
            }

            pub fn matcher(&self) -> Matcher {
                self.act_matcher.read().unwrap().clone()
            }

            pub(in crate::sch) fn set_matcher(&self, matcher: &Matcher) {
                *self.act_matcher.write().unwrap() = matcher.clone();
            }
        }
    };
}
