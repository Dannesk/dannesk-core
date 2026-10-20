

use serde_json::Value;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryList {
    XrpTransactions,
    XrpOrders,
    BtcTransactions,
}

impl HistoryList {

    pub fn kind(self) -> &'static str {
        match self {
            HistoryList::XrpTransactions => "transactions",
            HistoryList::XrpOrders => "orders",
            HistoryList::BtcTransactions => "transactions",
        }
    }

    pub fn total_field(self) -> &'static str {
        match self {
            HistoryList::XrpTransactions | HistoryList::BtcTransactions => "history_transactions",
            HistoryList::XrpOrders => "history_orders",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageStatus {
    #[default]
    Idle,
    Fetching,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistoryPage {
    pub status: PageStatus,
 
    pub total: Option<usize>,
  
    pub capped: bool,
    pub seq: u32,
}

impl HistoryPage {
    
    pub fn facts(&mut self, list: HistoryList, reply: &Value) {
        if let Some(n) = reply.get(list.total_field()).and_then(|v| v.as_u64()) {
            self.total = Some(n as usize);
        }
        if let Some(c) = reply.get("history_capped").and_then(|v| v.as_bool()) {
            self.capped = c;
        }
    }

    pub fn done(&mut self) {
        self.status = PageStatus::Idle;
    }

    pub fn start(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.status = PageStatus::Fetching;
        self.seq
    }

    
    pub fn fail(&mut self, seq: u32) {
        if self.status == PageStatus::Fetching && self.seq == seq {
            self.status = PageStatus::Failed;
        }
    }

    pub fn fail_any(&mut self) {
        if self.status == PageStatus::Fetching {
            self.status = PageStatus::Failed;
        }
    }

    pub fn more_than(&self, held: usize) -> bool {
        self.total.is_some_and(|t| held < t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reply_sets_the_facts_and_its_own_page_ends_the_request() {
        let mut p = HistoryPage::default();
        let seq = p.start();
        assert_eq!(p.status, PageStatus::Fetching);
        let reply = json!({ "history_orders": 7, "history_transactions": 40, "history_capped": true });
        p.facts(HistoryList::XrpOrders, &reply);
        assert_eq!(p.total, Some(7));
        assert!(p.capped);
        assert_eq!(p.status, PageStatus::Fetching, "still waiting for this list's page");
        p.done();
        assert_eq!(p.status, PageStatus::Idle);
        p.fail(seq);
        assert_eq!(p.status, PageStatus::Idle);
    }


    #[test]
    fn an_old_relay_leaves_the_page_unknown() {
        let mut p = HistoryPage::default();
        p.facts(HistoryList::BtcTransactions, &json!({ "balance": "0" }));
        assert_eq!(p.total, None);
        assert!(!p.more_than(0));
    }

    #[test]
    fn a_failure_names_the_request_it_is_for() {
        let mut p = HistoryPage::default();
        let first = p.start();
        let second = p.start();
        p.fail(first);
        assert_eq!(p.status, PageStatus::Fetching, "the first request was superseded");
        p.fail(second);
        assert_eq!(p.status, PageStatus::Failed);
        p.facts(HistoryList::XrpTransactions, &json!({ "history_transactions": 3 }));
        p.done();
        assert_eq!(p.status, PageStatus::Idle);
        assert!(p.more_than(2));
        assert!(!p.more_than(3));
    }
}
