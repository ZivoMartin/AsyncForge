use std::{collections::HashMap, ops::AddAssign};

use global_lib::ANONYMOUS;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Copy, Eq, PartialEq)]
struct Summary {
    nb_message_sent: usize,
    received: usize,
    awaited: Option<usize>,
}

impl AddAssign<Summary> for Summary {
    fn add_assign(&mut self, s: Summary) {
        self.nb_message_sent += s.nb_message_sent;
        self.received += s.received;
        if let Some(awaited) = self.awaited {
            self.awaited = Some(if let Some(other_awaited) = s.awaited {
                other_awaited + awaited
            } else {
                awaited
            });
        } else {
            self.awaited = s.awaited
        }
    }
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SummaryMessage {
    pub index: usize,
    pub nb_message_sent: usize,
}

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct Summaries {
    index: u16,
    n: Option<(usize, usize)>,
    summaries: HashMap<usize, Summary>,
}

impl Summaries {
    pub fn new(index: u16) -> Self {
        Self {
            index,
            ..Default::default()
        }
    }

    pub fn set_index(&mut self, index: u16) {
        self.index = index
    }

    pub fn get_messages(&self) -> Vec<(usize, SummaryMessage)> {
        (0..self.n.unwrap().0)
            .map(|i| {
                (
                    i,
                    SummaryMessage {
                        index: self.index as usize,
                        nb_message_sent: match self.summaries.get(&i) {
                            Some(s) => s.nb_message_sent,
                            None => 0,
                        },
                    },
                )
            })
            .collect()
    }

    pub fn add_messages_sent(&mut self, i: usize, n: usize) {
        self.summaries
            .entry(i)
            .and_modify(|summ| {
                summ.nb_message_sent += n;
            })
            .or_insert(Summary {
                nb_message_sent: n,
                received: 0,
                awaited: None,
            });
    }

    pub fn new_message_sent(&mut self, i: usize) {
        self.add_messages_sent(i, 1);
    }

    pub fn new_message_received(&mut self, i: usize) {
        self.summaries
            .entry(i)
            .and_modify(|summ| {
                summ.received += 1;
            })
            .or_insert(Summary {
                nb_message_sent: 0,
                received: 1,
                awaited: None,
            });
        self.check_summ(i);
    }

    pub fn summary_received(&mut self, received_summ: SummaryMessage) {
        self.summaries
            .entry(received_summ.index)
            .and_modify(|summ| {
                assert!(summ.awaited.is_none());
                summ.awaited = Some(received_summ.nb_message_sent);
            })
            .or_insert(Summary {
                nb_message_sent: 0,
                received: 0,
                awaited: Some(received_summ.nb_message_sent),
            });
        self.check_summ(received_summ.index);
    }

    fn check_summ(&mut self, index: usize) {
        let summ = match self.summaries.get(&index) {
            Some(summ) => summ,
            None => return,
        };
        if summ.awaited.is_some() && summ.received >= *summ.awaited.as_ref().unwrap() {
            assert!(
                summ.awaited == Some(summ.received),
                "NODE {} received to much message from {index}, received {}, when {} were awaited",
                self.index,
                summ.received,
                *summ.awaited.as_ref().unwrap()
            );
            if let Some((_, n)) = &mut self.n {
                *n -= 1;
            }
        }
    }

    pub fn set_n(&mut self, n: usize) {
        if self.n.is_some() {
            return;
        }
        self.n = Some((n, n));
        for i in 0..n {
            self.check_summ(i)
        }
    }

    pub fn is_done(&self) -> bool {
        // if self.n.is_some() && self.n.as_ref().unwrap().1 == 0 {
        //     println!("Node {} is OK", self.index);
        // } else if self.n.is_some() && self.n.as_ref().unwrap().1 == 1 {
        //     println!("Node {} almost done", self.index);
        // }
        // else if self.n.is_some() {
        // println!(
        // "Node {} remains {:?}",
        // self.index,
        // *self.n.as_ref().unwrap()
        // );
        //     println!(
        //         "Node {}, --------------------------- {:?}",
        //         self.index, self.summaries
        //     );
        // }

        self.n.is_some() && self.n.as_ref().unwrap().1 == 0
    }

    pub fn clear(&mut self) {
        self.summaries.clear();
        self.n = None;
    }

    pub fn index(&self) -> u16 {
        self.index
    }
}

impl AddAssign<Summaries> for Summaries {
    fn add_assign(&mut self, other: Summaries) {
        other.summaries.into_iter().for_each(|(i, s)| {
            self.summaries
                .entry(i)
                .and_modify(|my_s| my_s.add_assign(s))
                .or_insert(s);
            self.check_summ(i);
        });
    }
}

impl From<SummaryMessage> for Summaries {
    fn from(s: SummaryMessage) -> Summaries {
        let mut summaries = HashMap::with_capacity(1);
        summaries.insert(
            s.index,
            Summary {
                nb_message_sent: 0,
                received: 0,
                awaited: Some(s.nb_message_sent),
            },
        );
        Summaries {
            index: ANONYMOUS,
            n: None,
            summaries,
        }
    }
}
