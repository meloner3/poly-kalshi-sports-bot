use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::config::MentionMode;

use super::ArbitrageStorage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalState {
    pub mode: MentionMode,
    pub kill_switch: bool,
    pub loss_breaker: bool,
    pub breaker_day: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalPosition {
    pub id: i64,
    pub market_ticker: String,
    pub event_id: String,
    pub event_day: String,
    pub side: String,
    pub mode: String,
    pub model_version: String,
    pub p_hat: f64,
    pub p_lo: f64,
    pub p_hi: f64,
    pub contracts: i32,
    pub remaining_contracts: i32,
    pub entry_vwap: f64,
    pub entry_fees: f64,
    pub exit_vwap: Option<f64>,
    pub exit_fees: f64,
    pub net_pnl: Option<f64>,
    pub outcome_yes: Option<i32>,
    pub slippage: Option<f64>,
    pub predicted_edge: f64,
    pub close_time: Option<String>,
    pub client_order_id: String,
    pub order_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewDirectionalPosition<'a> {
    pub market_ticker: &'a str,
    pub event_id: &'a str,
    pub event_day: &'a str,
    pub side: &'a str,
    pub mode: &'a str,
    pub model_version: &'a str,
    pub p_hat: f64,
    pub p_lo: f64,
    pub p_hi: f64,
    pub contracts: i32,
    pub entry_vwap: f64,
    pub entry_fees: f64,
    pub slippage: f64,
    pub predicted_edge: f64,
    pub close_time: Option<&'a str>,
    pub client_order_id: &'a str,
    pub order_id: Option<&'a str>,
    pub status: &'a str,
}

#[derive(Debug, Clone)]
pub struct DirectionalOrderIntent {
    pub id: i64,
    pub position_id: i64,
    pub client_order_id: String,
    pub ticker: String,
    pub action: String,
    pub side: String,
    pub contracts: i32,
    pub limit_price_cents: i32,
    pub status: String,
    pub created_at: String,
}

pub struct NewDirectionalDecision<'a> {
    pub market_ticker: &'a str,
    pub event_id: Option<&'a str>,
    pub event_day: Option<&'a str>,
    pub side: Option<&'a str>,
    pub model_version: Option<&'a str>,
    pub p_hat: Option<f64>,
    pub p_lo: Option<f64>,
    pub p_hi: Option<f64>,
    pub yes_bid: Option<f64>,
    pub yes_ask: Option<f64>,
    pub net_edge: Option<f64>,
    pub vwap: Option<f64>,
    pub fee_per_contract: Option<f64>,
    pub contracts: i32,
    pub decision: &'a str,
    pub reason: &'a str,
}

impl ArbitrageStorage {
    pub(super) fn init_directional_tables(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS directional_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                mode TEXT NOT NULL DEFAULT 'off',
                is_configured INTEGER NOT NULL DEFAULT 0,
                current_model_version TEXT,
                kill_switch INTEGER NOT NULL DEFAULT 0,
                loss_breaker INTEGER NOT NULL DEFAULT 0,
                breaker_day TEXT,
                updated_at TEXT NOT NULL
            );
            INSERT OR IGNORE INTO directional_state (id, mode, kill_switch, loss_breaker, updated_at)
                VALUES (1, 'off', 0, 0, CURRENT_TIMESTAMP);
            CREATE TABLE IF NOT EXISTS directional_decisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                market_ticker TEXT NOT NULL,
                event_id TEXT,
                event_day TEXT,
                side TEXT,
                model_version TEXT,
                p_hat REAL,
                p_lo REAL,
                p_hi REAL,
                yes_bid REAL,
                yes_ask REAL,
                net_edge REAL,
                vwap REAL,
                fee_per_contract REAL,
                contracts INTEGER NOT NULL DEFAULT 0,
                decision TEXT NOT NULL,
                reason TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_directional_decision_created
                ON directional_decisions(created_at);
            CREATE TABLE IF NOT EXISTS directional_paper_positions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                market_ticker TEXT NOT NULL,
                event_id TEXT NOT NULL,
                event_day TEXT NOT NULL,
                side TEXT NOT NULL,
                mode TEXT NOT NULL,
                model_version TEXT NOT NULL,
                p_hat REAL NOT NULL,
                p_lo REAL NOT NULL,
                p_hi REAL NOT NULL,
                contracts INTEGER NOT NULL,
                remaining_contracts INTEGER NOT NULL,
                entry_vwap REAL NOT NULL,
                entry_fees REAL NOT NULL,
                exit_vwap REAL,
                exit_fees REAL NOT NULL DEFAULT 0,
                net_pnl REAL,
                outcome_yes INTEGER,
                slippage REAL,
                predicted_edge REAL NOT NULL DEFAULT 0,
                close_time TEXT,
                client_order_id TEXT NOT NULL UNIQUE,
                order_id TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_directional_position_status
                ON directional_paper_positions(mode, status, created_at);
            CREATE INDEX IF NOT EXISTS idx_directional_position_event
                ON directional_paper_positions(event_id, status);
            CREATE TABLE IF NOT EXISTS directional_realized_ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                position_id INTEGER NOT NULL,
                cashflow_kind TEXT NOT NULL,
                net_pnl REAL NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_directional_realized_created
                ON directional_realized_ledger(created_at);
            CREATE TABLE IF NOT EXISTS directional_order_intents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                position_id INTEGER NOT NULL,
                client_order_id TEXT NOT NULL UNIQUE,
                ticker TEXT NOT NULL,
                action TEXT NOT NULL,
                side TEXT NOT NULL,
                contracts INTEGER NOT NULL,
                limit_price_cents INTEGER NOT NULL,
                order_id TEXT,
                fill_count INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_directional_intent_status
                ON directional_order_intents(status,created_at);
            CREATE TABLE IF NOT EXISTS directional_decision_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                position_id INTEGER,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                reason TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );",
        )?;
        let _ = conn.execute(
            "ALTER TABLE directional_state ADD COLUMN is_configured INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE directional_state ADD COLUMN current_model_version TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE directional_paper_positions ADD COLUMN predicted_edge REAL NOT NULL DEFAULT 0",
            [],
        );
        // Paper and live cashflows must never be netted together: a paper loss must not
        // trip the live breaker's accounting and a paper win must not mask a live loss.
        let _ = conn.execute(
            "ALTER TABLE directional_realized_ledger ADD COLUMN mode TEXT NOT NULL DEFAULT 'paper'",
            [],
        );
        conn.execute(
            "UPDATE directional_realized_ledger SET mode = COALESCE(
                (SELECT p.mode FROM directional_paper_positions p
                 WHERE p.id = directional_realized_ledger.position_id), mode)",
            [],
        )?;
        Ok(())
    }

    pub fn initialize_directional_mode(&self, mode: MentionMode) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET mode=?1,is_configured=1,updated_at=?2 WHERE id=1 AND is_configured=0",
            params![mode_str(mode), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn directional_state(&self) -> Result<DirectionalState> {
        let conn = self.conn().lock();
        conn.query_row(
            "SELECT mode, kill_switch, loss_breaker, breaker_day, updated_at FROM directional_state WHERE id = 1",
            [],
            |row| Ok(DirectionalState {
                mode: parse_mode(&row.get::<_, String>(0)?),
                kill_switch: row.get::<_, i32>(1)? != 0,
                loss_breaker: row.get::<_, i32>(2)? != 0,
                breaker_day: row.get(3)?,
                updated_at: row.get(4)?,
            }),
        ).map_err(Into::into)
    }

    pub fn set_directional_mode(&self, mode: MentionMode) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET mode=?1, updated_at=?2 WHERE id=1",
            params![mode_str(mode), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_directional_model_version(&self, version: &str) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET current_model_version=?1,updated_at=?2 WHERE id=1",
            params![version, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn directional_model_version(&self) -> Result<Option<String>> {
        let conn = self.conn().lock();
        conn.query_row(
            "SELECT current_model_version FROM directional_state WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn set_directional_kill_switch(&self, enabled: bool) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET kill_switch=?1, updated_at=?2 WHERE id=1",
            params![enabled as i32, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn reset_directional_loss_breaker(&self) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET loss_breaker=0, breaker_day=NULL, updated_at=?1 WHERE id=1",
            params![Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn trip_directional_loss_breaker(&self, day: &str) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_state SET loss_breaker=1, breaker_day=?1, mode='off', updated_at=?2 WHERE id=1",
            params![day, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn save_directional_decision(&self, item: NewDirectionalDecision<'_>) -> Result<i64> {
        let conn = self.conn().lock();
        conn.execute(
            "INSERT INTO directional_decisions (
                market_ticker,event_id,event_day,side,model_version,p_hat,p_lo,p_hi,
                yes_bid,yes_ask,net_edge,vwap,fee_per_contract,contracts,decision,reason,created_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                item.market_ticker,
                item.event_id,
                item.event_day,
                item.side,
                item.model_version,
                item.p_hat,
                item.p_lo,
                item.p_hi,
                item.yes_bid,
                item.yes_ask,
                item.net_edge,
                item.vwap,
                item.fee_per_contract,
                item.contracts,
                item.decision,
                item.reason,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn create_directional_position(&self, item: NewDirectionalPosition<'_>) -> Result<i64> {
        let conn = self.conn().lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO directional_paper_positions (
                market_ticker,event_id,event_day,side,mode,model_version,p_hat,p_lo,p_hi,
                     contracts,remaining_contracts,entry_vwap,entry_fees,slippage,predicted_edge,close_time,
                client_order_id,order_id,status,created_at,updated_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?19)",
            params![item.market_ticker,item.event_id,item.event_day,item.side,item.mode,item.model_version,
                     item.p_hat,item.p_lo,item.p_hi,item.contracts,item.entry_vwap,item.entry_fees,item.slippage,
                     item.predicted_edge,item.close_time,item.client_order_id,item.order_id,item.status,Utc::now().to_rfc3339()],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO directional_decision_log(position_id,action,status,reason,payload_json,created_at)
             VALUES (?1,'entry',?2,'position created',?3,?4)",
            params![id,item.status,serde_json::json!({"client_order_id":item.client_order_id,"contracts":item.contracts}).to_string(),Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn record_directional_action(
        &self,
        id: i64,
        action: &str,
        status: &str,
        reason: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.conn().lock();
        let tx = conn.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO directional_decision_log(position_id,action,status,reason,payload_json,created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id,action,status,reason,payload.to_string(),now],
        )?;
        let decision = format!("{action}_{status}");
        tx.execute(
            "INSERT INTO directional_decisions(market_ticker,event_id,event_day,side,model_version,
             p_hat,p_lo,p_hi,contracts,decision,reason,created_at)
             SELECT market_ticker,event_id,event_day,side,model_version,p_hat,p_lo,p_hi,
             remaining_contracts,?2,?3,?4 FROM directional_paper_positions WHERE id=?1",
            params![id, decision, reason, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_directional_order_intent(
        &self,
        position_id: i64,
        client_order_id: &str,
        ticker: &str,
        action: &str,
        side: &str,
        contracts: i32,
        limit_price_cents: i32,
    ) -> Result<i64> {
        let conn = self.conn().lock();
        conn.execute("INSERT INTO directional_order_intents(position_id,client_order_id,ticker,action,side,contracts,limit_price_cents,status,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'submitting',?8,?8)",
            params![position_id,client_order_id,ticker,action,side,contracts,limit_price_cents,Utc::now().to_rfc3339()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_directional_order_intent(
        &self,
        intent_id: i64,
        order_id: Option<&str>,
        fill_count: i32,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute("UPDATE directional_order_intents SET order_id=?2,fill_count=?3,status=?4,error=?5,updated_at=?6 WHERE id=?1",
            params![intent_id,order_id,fill_count,status,error,Utc::now().to_rfc3339()])?;
        Ok(())
    }

    pub fn unresolved_directional_order_intents(&self) -> Result<Vec<DirectionalOrderIntent>> {
        let conn = self.conn().lock();
        let mut statement=conn.prepare("SELECT id,position_id,client_order_id,ticker,action,side,contracts,limit_price_cents,status,created_at FROM directional_order_intents WHERE status IN ('submitting','unresolved') ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(DirectionalOrderIntent {
                id: row.get(0)?,
                position_id: row.get(1)?,
                client_order_id: row.get(2)?,
                ticker: row.get(3)?,
                action: row.get(4)?,
                side: row.get(5)?,
                contracts: row.get(6)?,
                limit_price_cents: row.get(7)?,
                status: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn update_directional_entry_ack(
        &self,
        id: i64,
        order_id: Option<&str>,
        filled: i32,
        entry_vwap: f64,
        entry_fees: f64,
        status: &str,
    ) -> Result<()> {
        let conn = self.conn().lock();
        conn.execute(
            "UPDATE directional_paper_positions SET order_id=?2,
             contracts=CASE WHEN ?6 IN ('submitting','unresolved') THEN contracts ELSE ?3 END,
             remaining_contracts=CASE WHEN ?6 IN ('submitting','unresolved') THEN remaining_contracts ELSE ?3 END,
             entry_vwap=?4,entry_fees=?5,status=?6,updated_at=?7 WHERE id=?1",
            params![id,order_id,filled,entry_vwap,entry_fees,status,Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn close_directional_position(
        &self,
        id: i64,
        closed_contracts: i32,
        exit_vwap: f64,
        exit_fees: f64,
        net_pnl: f64,
        slippage: f64,
        status: &str,
    ) -> Result<()> {
        let conn = self.conn().lock();
        let tx = conn.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE directional_paper_positions SET remaining_contracts=MAX(remaining_contracts-?2,0),
             exit_vwap=?3,exit_fees=exit_fees+?4,net_pnl=COALESCE(net_pnl,0)+?5,
             slippage=COALESCE(slippage,0)+?6,status=?7,updated_at=?8 WHERE id=?1",
            params![id,closed_contracts,exit_vwap,exit_fees,net_pnl,slippage,status,now],
        )?;
        tx.execute("INSERT INTO directional_realized_ledger(position_id,cashflow_kind,net_pnl,mode,created_at) VALUES (?1,'exit',?2,COALESCE((SELECT mode FROM directional_paper_positions WHERE id=?1),'paper'),?3)",params![id,net_pnl,now])?;
        tx.commit()?;
        Ok(())
    }

    pub fn directional_positions_missing_outcome(&self) -> Result<Vec<DirectionalPosition>> {
        let conn = self.conn().lock();
        let mut statement = conn.prepare(
            "SELECT id,market_ticker,event_id,event_day,side,mode,model_version,p_hat,p_lo,p_hi,
             contracts,remaining_contracts,entry_vwap,entry_fees,exit_vwap,exit_fees,net_pnl,
             outcome_yes,slippage,predicted_edge,close_time,client_order_id,order_id,status,created_at,updated_at
             FROM directional_paper_positions WHERE outcome_yes IS NULL ORDER BY id"
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DirectionalPosition {
                id: row.get(0)?,
                market_ticker: row.get(1)?,
                event_id: row.get(2)?,
                event_day: row.get(3)?,
                side: row.get(4)?,
                mode: row.get(5)?,
                model_version: row.get(6)?,
                p_hat: row.get(7)?,
                p_lo: row.get(8)?,
                p_hi: row.get(9)?,
                contracts: row.get(10)?,
                remaining_contracts: row.get(11)?,
                entry_vwap: row.get(12)?,
                entry_fees: row.get(13)?,
                exit_vwap: row.get(14)?,
                exit_fees: row.get(15)?,
                net_pnl: row.get(16)?,
                outcome_yes: row.get(17)?,
                slippage: row.get(18)?,
                predicted_edge: row.get(19)?,
                close_time: row.get(20)?,
                client_order_id: row.get(21)?,
                order_id: row.get(22)?,
                status: row.get(23)?,
                created_at: row.get(24)?,
                updated_at: row.get(25)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn directional_position(&self, id: i64) -> Result<Option<DirectionalPosition>> {
        let conn = self.conn().lock();
        let mut statement=conn.prepare("SELECT id,market_ticker,event_id,event_day,side,mode,model_version,p_hat,p_lo,p_hi,contracts,remaining_contracts,entry_vwap,entry_fees,exit_vwap,exit_fees,net_pnl,outcome_yes,slippage,predicted_edge,close_time,client_order_id,order_id,status,created_at,updated_at FROM directional_paper_positions WHERE id=?1")?;
        let row = statement
            .query_row(params![id], |row| {
                Ok(DirectionalPosition {
                    id: row.get(0)?,
                    market_ticker: row.get(1)?,
                    event_id: row.get(2)?,
                    event_day: row.get(3)?,
                    side: row.get(4)?,
                    mode: row.get(5)?,
                    model_version: row.get(6)?,
                    p_hat: row.get(7)?,
                    p_lo: row.get(8)?,
                    p_hi: row.get(9)?,
                    contracts: row.get(10)?,
                    remaining_contracts: row.get(11)?,
                    entry_vwap: row.get(12)?,
                    entry_fees: row.get(13)?,
                    exit_vwap: row.get(14)?,
                    exit_fees: row.get(15)?,
                    net_pnl: row.get(16)?,
                    outcome_yes: row.get(17)?,
                    slippage: row.get(18)?,
                    predicted_edge: row.get(19)?,
                    close_time: row.get(20)?,
                    client_order_id: row.get(21)?,
                    order_id: row.get(22)?,
                    status: row.get(23)?,
                    created_at: row.get(24)?,
                    updated_at: row.get(25)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn resolve_directional_outcome(
        &self,
        id: i64,
        outcome_yes: bool,
        settlement_pnl: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn().lock();
        let tx = conn.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE directional_paper_positions SET outcome_yes=?2,
             net_pnl=CASE WHEN ?3 IS NULL THEN net_pnl ELSE COALESCE(net_pnl,0)+?3 END,
             remaining_contracts=CASE WHEN ?3 IS NULL THEN remaining_contracts ELSE 0 END,
             status=CASE WHEN ?3 IS NULL THEN status ELSE 'settled' END,updated_at=?4 WHERE id=?1",
            params![id, outcome_yes as i32, settlement_pnl, now],
        )?;
        if let Some(pnl) = settlement_pnl {
            tx.execute("INSERT INTO directional_realized_ledger(position_id,cashflow_kind,net_pnl,mode,created_at) VALUES (?1,'settlement',?2,COALESCE((SELECT mode FROM directional_paper_positions WHERE id=?1),'paper'),?3)",params![id,pnl,now])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn open_directional_positions(&self) -> Result<Vec<DirectionalPosition>> {
        let conn = self.conn().lock();
        let mut statement = conn.prepare(
            "SELECT id,market_ticker,event_id,event_day,side,mode,model_version,p_hat,p_lo,p_hi,
             contracts,remaining_contracts,entry_vwap,entry_fees,exit_vwap,exit_fees,net_pnl,
             outcome_yes,slippage,predicted_edge,close_time,client_order_id,order_id,status,created_at,updated_at
             FROM directional_paper_positions WHERE status IN ('open','submitting','unresolved') AND remaining_contracts > 0 ORDER BY id"
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DirectionalPosition {
                id: row.get(0)?,
                market_ticker: row.get(1)?,
                event_id: row.get(2)?,
                event_day: row.get(3)?,
                side: row.get(4)?,
                mode: row.get(5)?,
                model_version: row.get(6)?,
                p_hat: row.get(7)?,
                p_lo: row.get(8)?,
                p_hi: row.get(9)?,
                contracts: row.get(10)?,
                remaining_contracts: row.get(11)?,
                entry_vwap: row.get(12)?,
                entry_fees: row.get(13)?,
                exit_vwap: row.get(14)?,
                exit_fees: row.get(15)?,
                net_pnl: row.get(16)?,
                outcome_yes: row.get(17)?,
                slippage: row.get(18)?,
                predicted_edge: row.get(19)?,
                close_time: row.get(20)?,
                client_order_id: row.get(21)?,
                order_id: row.get(22)?,
                status: row.get(23)?,
                created_at: row.get(24)?,
                updated_at: row.get(25)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn has_unresolved_directional_submission(&self) -> Result<bool> {
        let conn = self.conn().lock();
        let count: i64 = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM directional_paper_positions WHERE mode='live' AND status IN ('submitting','unresolved'))+
             (SELECT COUNT(*) FROM directional_order_intents WHERE status IN ('submitting','unresolved'))",
            [], |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Exposure for ONE mode only (`paper` or `live`); the two books are never pooled.
    /// Live orders that are `submitting`/`unresolved` may already be filled at the
    /// exchange, so they consume risk budget exactly like open positions.
    pub fn directional_risk_totals(
        &self,
        ticker: &str,
        event_id: &str,
        day: &str,
        mode: &str,
    ) -> Result<(f64, f64, f64, f64, i64)> {
        let conn = self.conn().lock();
        let mut statement = conn.prepare(
            "SELECT COALESCE(SUM(CASE WHEN status IN ('open','submitting','unresolved') THEN entry_vwap*remaining_contracts+entry_fees ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN status IN ('open','submitting','unresolved') AND market_ticker=?1 THEN entry_vwap*remaining_contracts+entry_fees ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN status IN ('open','submitting','unresolved') AND event_id=?2 THEN entry_vwap*remaining_contracts+entry_fees ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN substr(created_at,1,10)=?3 THEN entry_vwap*contracts+entry_fees ELSE 0 END),0),
             SUM(CASE WHEN status IN ('open','submitting','unresolved') THEN 1 ELSE 0 END)
             FROM directional_paper_positions WHERE mode=?4"
        )?;
        let result = statement.query_row(params![ticker, event_id, day, mode], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        })?;
        Ok(result)
    }

    pub fn directional_day_pnl(&self, day: &str, mode: &str) -> Result<f64> {
        let conn = self.conn().lock();
        conn.query_row(
            "SELECT COALESCE(SUM(net_pnl),0) FROM directional_realized_ledger WHERE substr(created_at,1,10)=?1 AND mode=?2",
            params![day, mode], |row| row.get(0),
        ).map_err(Into::into)
    }

    pub fn directional_total_pnl(&self, mode: &str) -> Result<f64> {
        let conn = self.conn().lock();
        conn.query_row(
            "SELECT COALESCE(SUM(net_pnl),0) FROM directional_realized_ledger WHERE mode=?1",
            params![mode],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn directional_history(&self, limit: usize) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn().lock();
        let mut statement = conn.prepare(
            "SELECT id,market_ticker,event_id,event_day,side,mode,model_version,p_hat,p_lo,p_hi,
             contracts,remaining_contracts,entry_vwap,entry_fees,exit_vwap,exit_fees,net_pnl,
             outcome_yes,slippage,predicted_edge,close_time,client_order_id,order_id,status,created_at,updated_at
             FROM directional_paper_positions ORDER BY id DESC LIMIT ?1"
        )?;
        let rows = statement.query_map(params![limit.min(500) as i64], |row| {
            let mut object = serde_json::Map::new();
            for (index, key) in [
                "id",
                "market_ticker",
                "event_id",
                "event_day",
                "side",
                "mode",
                "model_version",
                "p_hat",
                "p_lo",
                "p_hi",
                "contracts",
                "remaining_contracts",
                "entry_vwap",
                "entry_fees",
                "exit_vwap",
                "exit_fees",
                "net_pnl",
                "outcome_yes",
                "slippage",
                "predicted_edge",
                "close_time",
                "client_order_id",
                "order_id",
                "status",
                "created_at",
                "updated_at",
            ]
            .iter()
            .enumerate()
            {
                let value: rusqlite::types::Value = row.get(index)?;
                object.insert(
                    (*key).to_string(),
                    match value {
                        rusqlite::types::Value::Null => serde_json::Value::Null,
                        rusqlite::types::Value::Integer(v) => serde_json::json!(v),
                        rusqlite::types::Value::Real(v) => serde_json::json!(v),
                        rusqlite::types::Value::Text(v) => serde_json::json!(v),
                        rusqlite::types::Value::Blob(_) => serde_json::Value::Null,
                    },
                );
            }
            Ok(serde_json::Value::Object(object))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn mode_str(mode: MentionMode) -> &'static str {
    match mode {
        MentionMode::Off => "off",
        MentionMode::Paper => "paper",
        MentionMode::Live => "live",
    }
}

fn parse_mode(mode: &str) -> MentionMode {
    match mode {
        "paper" => MentionMode::Paper,
        "live" => MentionMode::Live,
        _ => MentionMode::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_position<'a>(
        ticker: &'a str,
        mode: &'a str,
        status: &'a str,
        client_order_id: &'a str,
    ) -> NewDirectionalPosition<'a> {
        NewDirectionalPosition {
            market_ticker: ticker,
            event_id: "EVT",
            event_day: "26JUN15",
            side: "yes",
            mode,
            model_version: "m:abc",
            p_hat: 0.6,
            p_lo: 0.5,
            p_hi: 0.7,
            contracts: 10,
            entry_vwap: 0.40,
            entry_fees: 0.20,
            slippage: 0.0,
            predicted_edge: 0.05,
            close_time: None,
            client_order_id,
            order_id: None,
            status,
        }
    }

    #[tokio::test]
    async fn exposure_is_accounted_per_mode_and_counts_unresolved_orders() {
        let storage = ArbitrageStorage::new(":memory:").unwrap();
        let day = Utc::now().format("%Y-%m-%d").to_string();
        storage
            .create_directional_position(new_position("T-26JUN15-A", "paper", "open", "p1"))
            .unwrap();
        storage
            .create_directional_position(new_position("T-26JUN15-B", "live", "submitting", "l1"))
            .unwrap();

        let paper = storage
            .directional_risk_totals("T-26JUN15-A", "EVT", &day, "paper")
            .unwrap();
        let live = storage
            .directional_risk_totals("T-26JUN15-B", "EVT", &day, "live")
            .unwrap();
        // 10 contracts * 0.40 + 0.20 fees
        assert!((paper.0 - 4.2).abs() < 1e-9);
        assert_eq!(paper.4, 1);
        // A `submitting` live order may already be filled, so it consumes budget.
        assert!((live.0 - 4.2).abs() < 1e-9);
        assert_eq!(live.4, 1);
        // The paper book never sees live exposure on the other ticker, or vice versa.
        let paper_other = storage
            .directional_risk_totals("T-26JUN15-B", "EVT", &day, "paper")
            .unwrap();
        assert_eq!(paper_other.1, 0.0);
    }

    #[tokio::test]
    async fn realized_pnl_is_separated_by_mode() {
        let storage = ArbitrageStorage::new(":memory:").unwrap();
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let paper = storage
            .create_directional_position(new_position("T-26JUN15-A", "paper", "open", "p1"))
            .unwrap();
        let live = storage
            .create_directional_position(new_position("T-26JUN15-B", "live", "open", "l1"))
            .unwrap();
        storage
            .close_directional_position(paper, 10, 0.5, 0.1, 0.7, 0.0, "closed")
            .unwrap();
        storage
            .close_directional_position(live, 10, 0.3, 0.1, -1.9, 0.0, "closed")
            .unwrap();

        assert!((storage.directional_day_pnl(&day, "paper").unwrap() - 0.7).abs() < 1e-9);
        assert!((storage.directional_day_pnl(&day, "live").unwrap() + 1.9).abs() < 1e-9);
        assert!((storage.directional_total_pnl("paper").unwrap() - 0.7).abs() < 1e-9);
        assert!((storage.directional_total_pnl("live").unwrap() + 1.9).abs() < 1e-9);
    }
}
