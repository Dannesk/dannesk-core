use crate::btc_script_type::BtcScriptType;
use crate::channel::{CHANNEL, WSCommand};
use crate::ws::commands::bitcoin_auth::BitcoinWallet;
use dannesk_btc_codec::address::Address;
use dannesk_btc_codec::tx::{OutPoint, SEQUENCE_RBF, Transaction, TxIn, TxOut, Txid};
use dannesk_btc_codec::{hash160, script, sighash, sign};
use rand::RngExt;

#[derive(Debug, Clone, PartialEq)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub amount: u64,
    pub address: String,
}


pub fn kinds_of(coins: &[Utxo]) -> Vec<BtcScriptType> {
    let own = crate::btc_script_type::stored();
    coins
        .iter()
        .map(|c| BtcScriptType::of_address(&c.address).unwrap_or(own))
        .collect()
}


fn varint(n: usize) -> u64 {
    if n < 0xfd { 1 } else { 3 }
}

fn outputs_len(out_spk_lens: &[usize]) -> u64 {
    out_spk_lens.iter().map(|l| 8 + varint(*l) + *l as u64).sum()
}

fn sized(inputs: usize, ins: u64, witness: u64, outputs: usize, outs: u64) -> u64 {
    let base = 4 + varint(inputs) + ins + varint(outputs) + outs + 4;
    if witness == 0 {
        return base;
    }
    (base * 4 + 2 + witness).div_ceil(4)
}

pub fn vsize_for(inputs: &[BtcScriptType], out_spk_lens: &[usize]) -> u64 {
    let ins: u64 = inputs.iter().map(|k| 41 + k.script_sig_len()).sum();
    let witness: u64 = inputs.iter().map(|k| k.witness_len()).sum();
    sized(inputs.len(), ins, witness, out_spk_lens.len(), outputs_len(out_spk_lens))
}

/// vsize_for of every prefix of the inputs
pub fn prefix_vsizes<'a>(inputs: &'a [BtcScriptType], out_spk_lens: &'a [usize]) -> impl Iterator<Item = u64> + 'a {
    let outs = outputs_len(out_spk_lens);
    let (mut ins, mut witness) = (0u64, 0u64);
    inputs.iter().enumerate().map(move |(i, k)| {
        ins += 41 + k.script_sig_len();
        witness += k.witness_len();
        sized(i + 1, ins, witness, out_spk_lens.len(), outs)
    })
}


pub fn tier_sats_at(rate: f32, vsize: u64) -> u64 {
    let rate = (rate as f64 * 1e6).round() / 1e6;
    let raw = rate * vsize as f64;
    ((raw * 1e6).round() / 1e6).ceil() as u64
}

pub fn eligible_utxos(wallet: &str) -> Result<Vec<Utxo>, String> {
    let (set_wallet, set) = CHANNEL.btc_utxos_rx.borrow().clone();
    if set_wallet.as_deref() != Some(wallet) {
        return Err("Wallet data not loaded yet — try again shortly".to_string());
    }
  
    let own_send = |txid: &str| -> bool {
        CHANNEL
            .btc_transactions_rx
            .borrow()
            .transactions
            .get(txid)
            .is_some_and(|tx| tx.sender_addresses.iter().any(|a| a == wallet))
    };
    let mut coins: Vec<(u64, String, u32, u64, String)> = set
        .iter()
        .filter(|u| u.spent_by.is_none())
        .filter(|u| u.height > 0 || own_send(&u.txid))
        .map(|u| (u.height, u.txid.clone(), u.vout, u.sats, u.address.clone()))
        .collect();
 
    coins.sort_by(|a, b| {
        let key = |c: &(u64, String, u32, u64, String)| (c.0 == 0, c.0, c.1.clone(), c.2);
        key(a).cmp(&key(b))
    });
    Ok(coins
        .into_iter()
        .map(|(_, txid, vout, sats, address)| Utxo { txid, vout, amount: sats, address })
        .collect())
}


pub fn spendable_sats(wallet: &str) -> u64 {
    eligible_utxos(wallet)
        .map(|coins| coins.iter().map(|u| u.amount).sum())
        .unwrap_or(0)
}

pub fn calculate_unconfirmed_balance(utxos: &[Utxo]) -> u64 {
    let balance: u64 = utxos.iter().map(|utxo| utxo.amount).sum();
    balance
}

pub fn select_utxos(utxos: &[Utxo], amount: u64, fee: u64) -> Result<Vec<Utxo>, String> {
    let target = amount + fee;

    let unconfirmed_balance = calculate_unconfirmed_balance(utxos);
    if unconfirmed_balance < target {
        return Err(format!(
            "Insufficient unconfirmed balance: needed {} satoshis, available {} satoshis",
            target, unconfirmed_balance
        ));
    }

    let mut selected: Vec<Utxo> = Vec::new();
    let mut total: u64 = 0;
    for utxo in utxos.iter() {
        if total < target {
            selected.push(utxo.clone());
            total += utxo.amount;
        }
    }

    if total < target {
        return Err(format!(
            "Insufficient funds after UTXO selection: needed {} satoshis, got {} satoshis",
            target, total
        ));
    }

    Ok(selected)
}


fn input_for(utxo: &Utxo) -> Result<TxIn, String> {
    let txid: Txid = utxo
        .txid
        .parse()
        .map_err(|e| format!("Invalid txid {}: {}", utxo.txid, e))?;
    Ok(TxIn {
        previous_output: OutPoint { txid, vout: utxo.vout },
        script_sig: Vec::new(),
        sequence: SEQUENCE_RBF,
        witness: Vec::new(),
    })
}

pub async fn construct_transaction(
    wallet_obj: &BitcoinWallet,
    cmd: &WSCommand,
    _tx_type: &str,
    utxos: Vec<Utxo>,
    fee: String,
) -> Result<String, String> {
    let recipient = cmd
        .recipient
        .as_ref()
        .ok_or("Missing recipient".to_string())?;

    let amount_str = cmd.amount.as_ref().ok_or("Missing amount".to_string())?;

    let amount_btc = amount_str
        .parse::<f64>()
        .map_err(|e| format!("Failed to parse amount as float: {}", e))?;
    if amount_btc <= 0.0 {
        return Err("Amount must be greater than zero.".to_string());
    }
    let amount = (amount_btc * 100_000_000.0).round() as u64;
    if amount == 0 {
        return Err("Amount must be greater than zero after conversion.".to_string());
    }

    let fee = fee
        .parse::<u64>()
        .map_err(|e| format!("Failed to parse fee: {}", e))?;

    let unconfirmed_balance = calculate_unconfirmed_balance(&utxos);
    let target = amount + fee;
    if unconfirmed_balance < target {
        return Err(format!(
            "Insufficient unconfirmed balance: needed {} satoshis, available {} satoshis",
            target, unconfirmed_balance
        ));
    }

    // Select UTXOs
    let selected_utxos = select_utxos(&utxos, amount, fee)?;
    let total_input: u64 = selected_utxos.iter().map(|utxo| utxo.amount).sum();

    // Parse recipient address — mainnet only, any standard script.
    let recipient_addr =
        Address::parse(recipient).map_err(|e| format!("Invalid recipient address: {}", e))?;

    // Create transaction inputs
    let inputs: Vec<TxIn> = selected_utxos.iter().map(input_for).collect::<Result<Vec<_>, String>>()?;

    // Create transaction
    let mut tx = Transaction {
        version: 2,
        lock_time: 0,
        input: inputs,
        output: vec![TxOut {
            value: amount,
            script_pubkey: recipient_addr.script_pubkey(),
        }],
    };

    // Add change output if necessary
    let change = total_input - amount - fee;
    let change_kind = BtcScriptType::of_address(&wallet_obj.address).unwrap_or_else(crate::btc_script_type::stored);
    if change >= change_floor(change_kind, None) {
        let change_addr = Address::parse(&wallet_obj.address)
            .map_err(|e| format!("Invalid change address: {}", e))?;
        tx.output.push(TxOut {
            value: change,
            script_pubkey: change_addr.script_pubkey(),
        });
    } else if change > 0 {
        return Err(COINS_CHANGED.to_string());
    }

    if let Some(reviewed) = cmd.vsize {
        let lens: Vec<usize> = tx.output.iter().map(|o| o.script_pubkey.len()).collect();
        if vsize_for(&kinds_of(&selected_utxos), &lens) != reviewed {
            return Err(COINS_CHANGED.to_string());
        }
    }

    sign_inputs(&mut tx, &selected_utxos, wallet_obj)?;

    // Serialize transaction
    Ok(tx.to_hex())
}



fn sign_inputs(
    tx: &mut Transaction,
    coins: &[Utxo],
    wallet_obj: &BitcoinWallet,
) -> Result<(), String> {
    if tx.input.len() != coins.len() {
        return Err("input/coin count mismatch".to_string());
    }
    if let Some(coin) = coins.iter().find(|c| c.address != wallet_obj.address) {
        return Err(format!("No signing key for address {}", coin.address));
    }
    struct Owner {
        secret: dannesk_btc_codec::secp256k1::SecretKey,
        pubkey: [u8; 33],
        kind: BtcScriptType,
        spk: Vec<u8>,
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            self.secret.non_secure_erase();
        }
    }
    let secret = wallet_obj
        .secret_key()
        .ok_or_else(|| "No signing key for this wallet".to_string())?;
    let public_key = secret.public_key();
    let kind = BtcScriptType::of_address(&wallet_obj.address)
        .ok_or_else(|| format!("Cannot sign for address {}", wallet_obj.address))?;
    let owner = Owner { secret, pubkey: public_key.serialize(), kind, spk: kind.address(&public_key).script_pubkey() };
    let locked = Address::parse(&wallet_obj.address).ok().map(|a| a.script_pubkey());
    if locked.as_ref() != Some(&owner.spk) {
        return Err(format!("Signing key does not match address {}", wallet_obj.address));
    }

    let prevouts: Vec<TxOut> = coins
        .iter()
        .map(|c| TxOut { value: c.amount, script_pubkey: owner.spk.clone() })
        .collect();

    let mut signed: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::with_capacity(coins.len());
    for i in 0..tx.input.len() {
        match owner.kind {
            BtcScriptType::NativeSegwit | BtcScriptType::NestedSegwit => {
                let digest = sighash::p2wpkh(tx, i, &hash160(&owner.pubkey), coins[i].amount)
                    .map_err(|e| format!("Failed to compute sighash for input {}: {}", i, e))?;
                let sig = sign::ecdsa_sighash_all(&digest, &owner.secret);
                let witness = vec![sig, owner.pubkey.to_vec()];
                let script_sig = match owner.kind {
                    BtcScriptType::NestedSegwit => script::p2sh_p2wpkh_script_sig(&owner.pubkey),
                    _ => Vec::new(),
                };
                signed.push((script_sig, witness));
            }
            BtcScriptType::Legacy => {
                let digest = sighash::legacy(tx, i, &owner.spk)
                    .map_err(|e| format!("Failed to compute sighash for input {}: {}", i, e))?;
                let sig = sign::ecdsa_sighash_all(&digest, &owner.secret);
                signed.push((script::p2pkh_script_sig(&sig, &owner.pubkey), Vec::new()));
            }
            BtcScriptType::Taproot => {
                let digest = sighash::taproot_key_spend(tx, i, &prevouts)
                    .map_err(|e| format!("Failed to compute sighash for input {}: {}", i, e))?;
                let aux: [u8; 32] = rand::rng().random();
                let sig = sign::schnorr_key_spend(&digest, &owner.secret, &aux);
                signed.push((Vec::new(), vec![sig.to_vec()]));
            }
        }
    }
    for (input, (script_sig, witness)) in tx.input.iter_mut().zip(signed.into_iter()) {
        input.script_sig = script_sig;
        input.witness = witness;
    }
    Ok(())
}


/// bitcoin core's default dust relay fee in sat/vB.
pub const DUST_RELAY_FEE: f32 = 3.0;

/// bitcoin core's default discard fee in sat/vB
pub const DISCARD_FEE: f32 = 10.0;

/// coins change response.
pub const COINS_CHANGED: &str = "Your coins changed since review. Review the send again.";

/// bump change response.
pub const BUMP_CHANGED: &str = "Your coins changed since review. Review the bump again.";

const LATER_SEND: &str = "A later send already spends this one's leftover, so its fee can't be raised.";
const CANNOT_COVER: &str = "Not enough is left over to pay the higher fee, and no other coin can cover it.";
const NOT_OURS: &str = "Not every coin in this send belongs to this wallet.";
const NO_REBUILD: &str = "An output of this send can't be rebuilt.";

pub fn dust_size(spk_len: usize, witness: bool) -> u64 {
    9 + spk_len as u64 + if witness { 67 } else { 148 }
}

pub fn change_floor(kind: BtcScriptType, slowest: Option<f32>) -> u64 {
    let rate = slowest.map_or(DUST_RELAY_FEE, |r| r.clamp(DUST_RELAY_FEE, DISCARD_FEE));
    let witness = matches!(kind, BtcScriptType::NativeSegwit | BtcScriptType::Taproot);
    tier_sats_at(rate, dust_size(kind.spk_len(), witness))
}


#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementPlan {
 
    pub inputs: Vec<Utxo>,
    
    pub keep: Vec<(Vec<u8>, u64)>,
   
    pub change_sats: Option<u64>,
    pub change_spk: Option<Vec<u8>>,

    pub fee: u64,
    pub vsize: u64,
    pub added_input: bool,
}

impl ReplacementPlan {
    pub fn rate_sat_vb(&self) -> f64 {
        self.fee as f64 / self.vsize.max(1) as f64
    }
}


pub fn min_replacement_fee(info: &crate::channel::BtcRbfInfo, vsize: u64) -> u64 {
    let ceil_rate = |rate: f32| tier_sats_at(rate, vsize);
    let rule4 = info.fee_sats.saturating_add(ceil_rate(info.incremental_sat_vb));
    let relay_min = ceil_rate(info.min_sat_vb);
    let same_rate = (info.fee_sats as u128 * vsize as u128).div_ceil(info.vsize.max(1) as u128) as u64;
    rule4.max(relay_min).max(same_rate + 1)
}


pub fn plan_replacement(
    info: &crate::channel::BtcRbfInfo,
    own: &str,
    spares: &[Utxo],
    fee: u64,
) -> Result<ReplacementPlan, String> {
    let plan = replacement(info, own, spares, fee, Some(info.min_sat_vb))?;
    let floor = min_replacement_fee(info, plan.vsize);
    if plan.fee < floor {
        return Err(format!("at least {floor} sats to replace it"));
    }
    Ok(plan)
}

/// The replacement signed at exactly the fee the command carries. refuse instead of paying more.
pub fn replacement_at(
    info: &crate::channel::BtcRbfInfo,
    own: &str,
    spares: &[Utxo],
    fee: u64,
) -> Result<ReplacementPlan, String> {
    let plan = replacement(info, own, spares, fee, None)?;
    if plan.fee != fee {
        return Err(BUMP_CHANGED.to_string());
    }
    Ok(plan)
}

pub(crate) fn replacement(
    info: &crate::channel::BtcRbfInfo,
    own: &str,
    spares: &[Utxo],
    fee: u64,
    slowest: Option<f32>,
) -> Result<ReplacementPlan, String> {
    if info.descendants > 0 {
        return Err(LATER_SEND.to_string());
    }
    let mut inputs: Vec<Utxo> = Vec::with_capacity(info.inputs.len() + 1);
    for i in &info.inputs {
        let Some(address) = i.address.as_deref().filter(|a| *a == own) else {
            return Err(NOT_OURS.to_string());
        };
        inputs.push(Utxo { txid: i.txid.clone(), vout: i.vout, amount: i.sats, address: address.to_string() });
    }
    let is_ours = |o: &crate::channel::BtcRbfOutput| o.address.as_deref().is_some_and(|a| a == own);
    let change_idx = info.outputs.iter().rposition(is_ours);
    let mut keep: Vec<(Vec<u8>, u64)> = Vec::new();
    let mut change_spk: Option<Vec<u8>> = None;
    for (idx, o) in info.outputs.iter().enumerate() {
        let spk = match (&o.spk, &o.address) {
            (Some(hex), _) => hex::decode(hex).map_err(|_| NO_REBUILD.to_string())?,
            (None, Some(addr)) => Address::parse(addr)
                .ok()
                .map(|a| a.script_pubkey())
                .ok_or_else(|| NO_REBUILD.to_string())?,
            (None, None) => return Err(NO_REBUILD.to_string()),
        };
        if Some(idx) == change_idx {
            change_spk = Some(spk);
        } else {
            keep.push((spk, o.sats));
        }
    }
    let keep_sum: u64 = keep.iter().map(|(_, v)| *v).sum();
    let mut in_sum: u64 = inputs.iter().map(|u| u.amount).sum();
    let mut room = in_sum as i128 - keep_sum as i128 - fee as i128;
    let mut added_input = false;
    if room < 0 {
        // The oldest spare coin that covers what the leftover cannot.
        let Some(extra) = spares.iter().find(|c| c.amount as i128 >= -room) else {
            return Err(CANNOT_COVER.to_string());
        };
        inputs.push(extra.clone());
        in_sum = in_sum.saturating_add(extra.amount);
        room += extra.amount as i128;
        added_input = true;
    }
    let change_kind = BtcScriptType::of_address(own).unwrap_or_else(crate::btc_script_type::stored);
    let change_sats = (room as u64 >= change_floor(change_kind, slowest)).then_some(room as u64);
    let effective_fee = in_sum - keep_sum - change_sats.unwrap_or(0);

    let mut out_lens: Vec<usize> = keep.iter().map(|(spk, _)| spk.len()).collect();
    if change_sats.is_some() {
        out_lens.push(change_spk.as_ref().map_or(change_kind.spk_len(), |s| s.len()));
    }
    let vsize = vsize_for(&kinds_of(&inputs), &out_lens);
    Ok(ReplacementPlan {
        inputs,
        keep,
        change_sats,
        change_spk,
        fee: effective_fee,
        vsize,
        added_input,
    })
}



pub fn rbf_info_for(txid: &str) -> Result<crate::channel::BtcRbfInfo, String> {
    let txs = CHANNEL.btc_transactions_rx.borrow();
    let Some(rec) = txs.transactions.get(txid) else {
        return Err("This send isn't in view.".to_string());
    };
    if rec.status != crate::channel::BitcoinTransactionStatus::Pending {
        return Err("This send is no longer pending.".to_string());
    }
    let details = || "This send's details aren't loaded yet.".to_string();
    if rec.inputs.is_empty() || rec.outputs.is_empty() {
        return Err(details());
    }
    let vsize = rec.vsize.filter(|v| *v > 0).ok_or_else(details)?;
    let fee_sats: u64 = rec.fees.parse().ok().filter(|f| *f > 0).ok_or_else(details)?;
    let node = CHANNEL.btc_node_rx.borrow().clone();
    let rates = || "Fee rates from the node aren't in yet.".to_string();
    let incremental_sat_vb = node.incremental_sat_vb.ok_or_else(rates)?;
    let min_sat_vb = node.tiers.ok_or_else(rates)?[0];
    let descendants = txs
        .transactions
        .values()
        .filter(|t| matches!(t.status, crate::channel::BitcoinTransactionStatus::Pending))
        .filter(|t| t.inputs.iter().any(|i| i.txid == txid))
        .count() as u32;
    Ok(crate::channel::BtcRbfInfo {
        txid: txid.to_string(),
        vsize,
        fee_sats,
        inputs: rec.inputs.clone(),
        outputs: rec.outputs.clone(),
        incremental_sat_vb,
        min_sat_vb,
        descendants,
    })
}


pub fn quote_replacement(
    info: &crate::channel::BtcRbfInfo,
    own: &str,
    spares: &[Utxo],
    rate: f32,
) -> Result<ReplacementPlan, String> {
    let mut fee = tier_sats_at(rate, info.vsize);
    let mut plan = plan_replacement(info, own, spares, fee)?;
    for _ in 0..3 {
        let next = tier_sats_at(rate, plan.vsize);
        if next == fee || next <= plan.fee && plan.change_sats.is_none() {
            break;
        }
        fee = next;
        plan = plan_replacement(info, own, spares, fee)?;
    }
    Ok(plan)
}


/// The confirmed coins a bump can add, oldest first.
pub fn spare_coins(wallet: &str, info: &crate::channel::BtcRbfInfo) -> Vec<Utxo> {
    let guard = CHANNEL.btc_utxos_rx.borrow();
    let (set_wallet, set) = &*guard;
    if set_wallet.as_deref() != Some(wallet) {
        return Vec::new();
    }
    let mut coins: Vec<&crate::channel::BtcUtxo> = set
        .iter()
        .filter(|u| u.height > 0 && u.spent_by.is_none() && u.txid != info.txid)
        .filter(|u| !info.inputs.iter().any(|i| i.txid == u.txid && i.vout == u.vout))
        .collect();
    coins.sort_by(|a, b| (a.height, &a.txid, a.vout).cmp(&(b.height, &b.txid, b.vout)));
    coins
        .into_iter()
        .map(|u| Utxo { txid: u.txid.clone(), vout: u.vout, amount: u.sats, address: u.address.clone() })
        .collect()
}


pub fn construct_replacement(
    wallet_obj: &BitcoinWallet,
    plan: &ReplacementPlan,
) -> Result<String, String> {
    let inputs: Vec<TxIn> = plan.inputs.iter().map(input_for).collect::<Result<Vec<_>, String>>()?;
    let mut output: Vec<TxOut> = plan
        .keep
        .iter()
        .map(|(spk, sats)| TxOut { value: *sats, script_pubkey: spk.clone() })
        .collect();
    if let Some(sats) = plan.change_sats {
        let spk = match &plan.change_spk {
            Some(spk) => spk.clone(),
            None => Address::parse(&wallet_obj.address)
                .map_err(|e| format!("Invalid change address: {}", e))?
                .script_pubkey(),
        };
        output.push(TxOut { value: sats, script_pubkey: spk });
    }
    let mut tx = Transaction { version: 2, lock_time: 0, input: inputs, output };
    sign_inputs(&mut tx, &plan.inputs, wallet_obj)?;
    Ok(tx.to_hex())
}

#[cfg(test)]
mod sign_tests {
    use super::*;
    use crate::btc_script_type::BtcScriptType;
    use dannesk_btc_codec::secp256k1::ecdsa::Signature as EcdsaSignature;
    use dannesk_btc_codec::secp256k1::{Message, PublicKey, SecretKey, XOnlyPublicKey, schnorr};

    fn wallet_of(kind: BtcScriptType, coins: usize) -> (BitcoinWallet, Vec<Utxo>) {
        let bytes = [7u8; 32];
        let sk = SecretKey::from_secret_bytes(bytes).unwrap();
        let address = kind.address(&sk.public_key()).to_string();
        let coins = (0..coins)
            .map(|i| Utxo {
                txid: format!("{:02x}", 0x11 + i).repeat(32),
                vout: i as u32,
                amount: 50_000 + i as u64,
                address: address.clone(),
            })
            .collect();
        (BitcoinWallet::for_test(address, bytes), coins)
    }

    fn unsigned(coins: &[Utxo]) -> Transaction {
        Transaction {
            version: 2,
            lock_time: 0,
            input: coins.iter().map(|c| input_for(c).unwrap()).collect(),
            output: vec![TxOut {
                value: 100_000,
                script_pubkey: hex::decode("0014aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            }],
        }
    }

    fn prevouts(coins: &[Utxo]) -> Vec<TxOut> {
        coins
            .iter()
            .map(|c| TxOut {
                value: c.amount,
                script_pubkey: Address::parse(&c.address).unwrap().script_pubkey(),
            })
            .collect()
    }

    #[test]
    fn every_family_signs_and_verifies() {
        for kind in BtcScriptType::IMPORT {
            let (wallet, coins) = wallet_of(kind, 2);
            let mut tx = unsigned(&coins);
            sign_inputs(&mut tx, &coins, &wallet).unwrap();
            let prevouts = prevouts(&coins);
            for i in 0..coins.len() {
                let input = &tx.input[i];
                match kind {
                    BtcScriptType::Taproot => {
                        assert!(input.script_sig.is_empty());
                        assert_eq!(input.witness.len(), 1);
                        assert_eq!(input.witness[0].len(), 64, "SIGHASH_DEFAULT: no hash type byte");
                        let sig = schnorr::Signature::from_byte_array(input.witness[0][..].try_into().unwrap());
                        let digest = sighash::taproot_key_spend(&tx, i, &prevouts).unwrap();
                        let output_key =
                            XOnlyPublicKey::from_byte_array(prevouts[i].script_pubkey[2..].try_into().unwrap()).unwrap();
                        sig.verify(&digest, &output_key).unwrap();
                    }
                    BtcScriptType::Legacy => {
                        assert!(input.witness.is_empty());
                        let items = script::pushes(&input.script_sig).unwrap();
                        assert_eq!(items.len(), 2);
                        let (der, ty) = items[0].split_at(items[0].len() - 1);
                        assert_eq!(ty, [sighash::SIGHASH_ALL]);
                        let pk = PublicKey::from_slice(&items[1]).unwrap();
                        assert_eq!(Address::p2pkh(&pk).to_string(), coins[i].address);
                        let digest = sighash::legacy(&tx, i, &prevouts[i].script_pubkey).unwrap();
                        EcdsaSignature::from_der(der).unwrap().verify(Message::from_digest(digest), &pk).unwrap();
                    }
                    BtcScriptType::NativeSegwit | BtcScriptType::NestedSegwit => {
                        assert_eq!(input.witness.len(), 2);
                        let (der, ty) = input.witness[0].split_at(input.witness[0].len() - 1);
                        assert_eq!(ty, [sighash::SIGHASH_ALL]);
                        let pk = PublicKey::from_slice(&input.witness[1]).unwrap();
                        assert_eq!(kind.address(&pk).to_string(), coins[i].address);
                        let digest = sighash::p2wpkh(&tx, i, &hash160(&pk.serialize()), coins[i].amount).unwrap();
                        EcdsaSignature::from_der(der).unwrap().verify(Message::from_digest(digest), &pk).unwrap();
                        match kind {
                            BtcScriptType::NestedSegwit => {
                                let redeem = script::p2wpkh(&hash160(&pk.serialize()));
                                assert_eq!(script::pushes(&input.script_sig).unwrap(), vec![redeem]);
                            }
                            _ => assert!(input.script_sig.is_empty()),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_alone_is_unchanged() {
        let (wallet, coins) = wallet_of(BtcScriptType::NativeSegwit, 2);
        let mut tx = unsigned(&coins);
        sign_inputs(&mut tx, &coins, &wallet).unwrap();
        for input in &tx.input {
            assert!(input.script_sig.is_empty());
            assert_eq!(input.witness.len(), 2);
        }
    }

    #[tokio::test]
    async fn a_leftover_under_the_floor_is_refused_not_paid() {
        let (wallet, coins) = wallet_of(BtcScriptType::NativeSegwit, 1);
        let send = |amount: &str| WSCommand {
            recipient: Some("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string()),
            amount: Some(amount.to_string()),
            ..Default::default()
        };
        let tiny = construct_transaction(&wallet, &send("0.00048700"), "BTC", coins.clone(), "1100".to_string()).await;
        assert_eq!(tiny, Err(COINS_CHANGED.to_string()), "a 200 sat leftover was kept or paid");
        let exact = construct_transaction(&wallet, &send("0.00048900"), "BTC", coins.clone(), "1100".to_string()).await.unwrap();
        assert_eq!(Transaction::from_hex(&exact).unwrap().output.len(), 1);
        let hex = construct_transaction(&wallet, &send("0.00040000"), "BTC", coins.clone(), "1100".to_string()).await.unwrap();
        let tx = Transaction::from_hex(&hex).unwrap();
        assert_eq!(tx.output.len(), 2);
        assert_eq!(tx.output[1].value, 8_900);
        assert_eq!(tx.output[1].script_pubkey, Address::parse(&coins[0].address).unwrap().script_pubkey());
    }

    async fn built_as_quoted(wallet: &BitcoinWallet, kind: BtcScriptType, coins: &[Utxo], tiers: [f32; 4], typed: u64, max: bool) -> usize {
        use crate::utils::btc_send::{price, BtcSendDraft};
        let to = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let total: u64 = coins.iter().map(|c| c.amount).sum();
        let btc = |sats: u64| format!("{:.8}", sats as f64 / 1e8);
        let draft = BtcSendDraft { recipient: to.to_string(), amount: btc(typed), custom: Some("1.5".to_string()), max };
        let quote = price(&draft, kind, Some(coins), Some(tiers));
        let mut built = 0;
        for t in quote.tiers.unwrap().into_iter().chain(quote.custom) {
            let amount = if max { t.max } else { typed };
            if amount + t.fee > total {
                continue;
            }
            let cmd = WSCommand { recipient: Some(to.to_string()), amount: Some(btc(amount)), vsize: Some(t.vsize), ..Default::default() };
            let hex = construct_transaction(wallet, &cmd, "BTC", coins.to_vec(), t.fee.to_string()).await.unwrap();
            let tx = Transaction::from_hex(&hex).unwrap();
            let spent: u64 = tx.input.iter().map(|i| coins[i.previous_output.vout as usize].amount).sum();
            let sent: u64 = tx.output.iter().map(|o| o.value).sum();
            assert_eq!(spent - sent, t.fee, "{kind:?} {amount} sat at {} sat/vB", t.rate);
            let inputs = tx.input.len() as u64;
            let slack = 1 + if kind == BtcScriptType::Legacy { 2 * inputs } else { inputs / 2 };
            assert!(tx.vsize() <= t.vsize && t.vsize - tx.vsize() <= slack, "{kind:?} {amount} sat at {} sat/vB built {} vB, quoted {}", t.rate, tx.vsize(), t.vsize);
            built += 1;
        }
        built
    }

    #[tokio::test]
    async fn every_quote_is_the_fee_the_builder_pays() {
        use BtcScriptType::*;
        let (wallet, coins) = wallet_of(NativeSegwit, 2);
        let mut built = 0;
        for (typed, max) in [(10_000u64, false), (49_500, false), (49_700, false), (99_800, false), (0, true)] {
            built += built_as_quoted(&wallet, NativeSegwit, &coins, [1.0, 2.0, 5.0, 10.0], typed, max).await;
        }
        built += built_as_quoted(&wallet, NativeSegwit, &coins, [12.0; 4], 47_500, false).await;
        let (wallet, mut small) = wallet_of(NativeSegwit, 80);
        for coin in small.iter_mut() {
            coin.amount = 300;
        }
        small[79].amount = 1_000_000;
        built += built_as_quoted(&wallet, NativeSegwit, &small, [4.0; 4], 2_000, false).await;
        assert_eq!(built, 31);

        for kind in [Taproot, NestedSegwit, Legacy] {
            let (wallet, coins) = wallet_of(kind, 2);
            let mut built = 0;
            for (typed, max) in [(10_000u64, false), (49_500, false), (49_700, false), (0, true)] {
                built += built_as_quoted(&wallet, kind, &coins, [1.0, 2.0, 5.0, 10.0], typed, max).await;
            }
            built += built_as_quoted(&wallet, kind, &coins, [12.0; 4], 47_500, false).await;
            assert_eq!(built, 25, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn a_send_that_no_longer_has_the_reviewed_size_is_refused() {
        let (wallet, mut coins) = wallet_of(BtcScriptType::NativeSegwit, 2);
        coins[0].amount = 30_000;
        coins[1].amount = 30_000;
        let send = |vsize: u64| WSCommand {
            recipient: Some("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string()),
            amount: Some("0.00020000".to_string()),
            vsize: Some(vsize),
            ..Default::default()
        };
        let one = vsize_for(&[BtcScriptType::NativeSegwit], &[22, 22]);
        assert!(construct_transaction(&wallet, &send(one), "BTC", coins.clone(), "141".to_string()).await.is_ok());
        let split: Vec<Utxo> = [9_000u64, 9_000, 42_000]
            .iter()
            .enumerate()
            .map(|(i, s)| Utxo { amount: *s, vout: i as u32 + 5, ..coins[0].clone() })
            .collect();
        assert_eq!(
            construct_transaction(&wallet, &send(one), "BTC", split, "141".to_string()).await,
            Err(COINS_CHANGED.to_string()),
            "three coins at the fee of one would pay a lower rate than reviewed"
        );
    }

    #[test]
    fn a_coin_of_another_address_fails_the_whole_build() {
        let (wallet, mut coins) = wallet_of(BtcScriptType::Taproot, 1);
        coins.push(Utxo { txid: "22".repeat(32), vout: 0, amount: 1, address: "bc1qyfxmsjaaaaaaaaaaaaaaaaaaaaaaaaaaamh06f67".into() });
        let mut tx = unsigned(&coins);
        assert!(sign_inputs(&mut tx, &coins, &wallet).is_err());
    }

    #[test]
    fn a_key_that_does_not_reproduce_its_address_is_refused() {
        let (_, coins) = wallet_of(BtcScriptType::Legacy, 1);
        let wallet = BitcoinWallet::for_test(coins[0].address.clone(), [9u8; 32]);
        let mut tx = unsigned(&coins);
        let err = sign_inputs(&mut tx, &coins, &wallet).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }
}

#[cfg(test)]
mod rbf_tests {
    use super::*;
    use crate::channel::{BtcRbfInfo, BtcRbfInput, BtcRbfOutput};

    const OWN: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
    const THEM_SPK: &str = "0014aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OWN_SPK: &str = "0014bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn info() -> BtcRbfInfo {
        BtcRbfInfo {
            txid: "aa".repeat(32),
            vsize: 141,
            fee_sats: 141,
            inputs: vec![BtcRbfInput { txid: "11".repeat(32), vout: 0, sats: 100_000, address: Some(OWN.into()) }],
            outputs: vec![
                BtcRbfOutput { vout: 0, sats: 60_000, address: Some("them".into()), spk: Some(THEM_SPK.into()) },
                BtcRbfOutput { vout: 1, sats: 39_859, address: Some(OWN.into()), spk: Some(OWN_SPK.into()) },
            ],
            incremental_sat_vb: 1.0,
            min_sat_vb: 1.0,
            descendants: 0,
        }
    }

    #[test]
    fn the_change_shrinks_and_the_recipient_is_untouched() {
        let plan = plan_replacement(&info(), OWN, &[], 1_000).unwrap();
        assert_eq!(plan.fee, 1_000);
        assert_eq!(plan.change_sats, Some(39_000));
        assert_eq!(plan.keep, vec![(hex::decode(THEM_SPK).unwrap(), 60_000)]);
        assert_eq!(plan.vsize, 141);
        assert!(!plan.added_input);
    }

    #[test]
    fn bip125_floor_is_fee_plus_incremental_and_a_higher_rate() {
        assert!(plan_replacement(&info(), OWN, &[], 281).is_err());
        assert_eq!(plan_replacement(&info(), OWN, &[], 282).unwrap().fee, 282);
        assert_eq!(min_replacement_fee(&info(), 141), 282);
        let mut i = info(); i.min_sat_vb = 5.0;
        assert_eq!(min_replacement_fee(&i, 141), 705);
    }

    #[test]
    fn change_under_the_floor_is_absorbed_into_the_fee() {
        let kept = plan_replacement(&info(), OWN, &[], 39_500).unwrap();
        assert_eq!((kept.change_sats, kept.fee, kept.vsize), (Some(500), 39_500, 141));
        let plan = plan_replacement(&info(), OWN, &[], 39_800).unwrap();
        assert_eq!((plan.change_sats, plan.fee, plan.vsize), (None, 40_000, 110));
        let mut busy = info();
        busy.min_sat_vb = 10.0;
        let plan = plan_replacement(&busy, OWN, &[], 39_500).unwrap();
        assert_eq!((plan.change_sats, plan.fee), (None, 40_000));
    }

    #[test]
    fn signing_builds_the_reviewed_replacement_at_the_commands_fee() {
        let mut busy = info();
        busy.min_sat_vb = 10.0;
        let kept = plan_replacement(&info(), OWN, &[], 39_500).unwrap();
        assert_eq!(replacement_at(&busy, OWN, &[], kept.fee), Ok(kept), "a fee rise after review changed the bump");
        let absorbed = plan_replacement(&info(), OWN, &[], 39_800).unwrap();
        assert_eq!(replacement_at(&busy, OWN, &[], absorbed.fee), Ok(absorbed));
        assert_eq!(replacement_at(&info(), OWN, &[], 39_800), Err(BUMP_CHANGED.to_string()));
        assert_eq!(replacement_at(&info(), OWN, &[], 39_706).map(|p| p.change_sats), Ok(Some(294)));
        assert_eq!(replacement_at(&info(), OWN, &[], 39_707), Err(BUMP_CHANGED.to_string()));
    }

    #[test]
    fn signing_leaves_the_replacement_minimum_to_review_and_the_node() {
        let reviewed = plan_replacement(&info(), OWN, &[], 282).unwrap();
        let mut risen = info();
        risen.min_sat_vb = 5.0;
        assert!(plan_replacement(&risen, OWN, &[], 282).is_err());
        assert_eq!(replacement_at(&risen, OWN, &[], 282), Ok(reviewed));
    }

    fn spare(txid: &str, amount: u64) -> Utxo {
        Utxo { txid: txid.repeat(32), vout: 1, amount, address: OWN.into() }
    }

    #[test]
    fn a_spare_coin_funds_what_the_change_cannot() {
        assert!(plan_replacement(&info(), OWN, &[], 45_000).is_err());
        let plan = plan_replacement(&info(), OWN, &[spare("22", 50_000)], 45_000).unwrap();
        assert!(plan.added_input);
        assert_eq!(plan.inputs.len(), 2);
        assert_eq!(plan.change_sats, Some(150_000 - 60_000 - 45_000));
        assert_eq!(plan.vsize, 209);
    }

    #[test]
    fn the_added_coin_is_the_oldest_one_big_enough_to_pay() {
        let spares = [spare("21", 2_000), spare("22", 50_000), spare("23", 90_000)];
        let plan = plan_replacement(&info(), OWN, &spares, 45_000).unwrap();
        assert_eq!(plan.inputs[1].txid, "22".repeat(32), "the 2,000 sat coin can't cover a 5,000 sat gap");
        assert_eq!(plan.change_sats, Some(150_000 - 60_000 - 45_000));
        assert_eq!(replacement_at(&info(), OWN, &spares, plan.fee), Ok(plan));
        assert_eq!(plan_replacement(&info(), OWN, &spares[..1], 45_000), Err(CANNOT_COVER.to_string()));
    }

    #[test]
    fn a_coin_that_leaves_less_than_the_floor_is_reviewed_and_signed_alike() {
        let spares = [spare("22", 5_200)];
        let reviewed = plan_replacement(&info(), OWN, &spares, 45_000).unwrap();
        assert_eq!((reviewed.change_sats, reviewed.fee, reviewed.vsize), (None, 45_200, 178));
        assert_eq!(replacement_at(&info(), OWN, &spares, reviewed.fee), Ok(reviewed));
    }

    #[test]
    fn descendants_and_foreign_inputs_refuse_with_their_reasons() {
        let mut i = info(); i.descendants = 1;
        assert_eq!(plan_replacement(&i, OWN, &[], 1_000), Err(LATER_SEND.to_string()));
        assert_eq!(replacement_at(&i, OWN, &[], 1_000), Err(LATER_SEND.to_string()));
        let mut i = info(); i.inputs[0].address = Some("bc1qsomeoneelse".into());
        assert_eq!(plan_replacement(&i, OWN, &[], 1_000), Err(NOT_OURS.to_string()));
        let mut max = info();
        max.outputs.pop();
        max.fee_sats = 40_000;
        assert_eq!(replacement_at(&max, OWN, &[], 41_000), Err(CANNOT_COVER.to_string()), "a send with nothing left over");
    }

    #[test]
    fn an_output_script_is_rebuilt_from_its_address() {
        let mut i = info();
        i.outputs[0] = BtcRbfOutput {
            vout: 0,
            sats: 60_000,
            address: Some("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".into()),
            spk: None,
        };
        let plan = plan_replacement(&i, OWN, &[], 1_000).unwrap();
        let want = Address::parse("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
            .unwrap()
            .script_pubkey();
        assert_eq!(plan.keep, vec![(want, 60_000)]);
    }

    #[test]
    fn a_tier_is_priced_at_the_replacement_size() {
        let plan = quote_replacement(&info(), OWN, &[], 10.0).unwrap();
        assert_eq!(plan.fee, 1_410);
        assert!(quote_replacement(&info(), OWN, &[], 1.0).is_err());
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    use crate::btc_script_type::BtcScriptType;

    
    #[test]
    fn a_tier_costs_its_rate_and_nothing_else() {
        assert_eq!(tier_sats_at(1.0, 140), 140);
        assert_eq!(tier_sats_at(0.1, 140), 14);
        assert_eq!(tier_sats_at(3.0, 140), 420);
        assert_eq!(tier_sats_at(2.5, 140), 350);
        assert_eq!(tier_sats_at(1.001, 140), 141);
        assert_eq!([0.30, 1.10, 1.20].map(|r| tier_sats_at(r, 110)), [33, 121, 132]);
    }

 
    #[test]
    fn vsize_is_exact_for_the_shapes_we_build() {
        use BtcScriptType::*;
        assert_eq!(vsize_for(&[NativeSegwit], &[22, 22]), 141);
        assert_eq!(vsize_for(&[NativeSegwit; 2], &[22, 22]), 209);
        assert_eq!(vsize_for(&[NativeSegwit; 3], &[22, 22]), 278);
        assert_eq!(vsize_for(&[NativeSegwit], &[22]), 110);
        assert_eq!(vsize_for(&[NativeSegwit], &[25, 22]), 144);
        assert_eq!(vsize_for(&[NativeSegwit], &[34, 22]), 153);
    }


    #[test]
    fn the_running_size_of_every_prefix_is_vsize_for_it() {
        use BtcScriptType::*;
        let mixes: [&[BtcScriptType]; 4] = [&[NativeSegwit], &[Legacy, Legacy], &[Legacy, Taproot, NestedSegwit], &[Taproot, NativeSegwit]];
        let shapes: [&[usize]; 3] = [&[22], &[22, 34], &[25, 22, 23]];
        for outs in shapes {
            for mix in mixes {
                let inputs: Vec<BtcScriptType> = mix.iter().copied().cycle().take(300).collect();
                for (i, size) in prefix_vsizes(&inputs, outs).enumerate() {
                    assert_eq!(size, vsize_for(&inputs[..=i], outs), "{mix:?} {outs:?} at {} inputs", i + 1);
                }
            }
        }
    }

    #[test]
    fn vsize_per_input_family() {
        use BtcScriptType::*;
        assert_eq!(vsize_for(&[Taproot], &[22, 34]), 142);
        assert_eq!(vsize_for(&[NestedSegwit], &[22, 23]), 165);
        assert_eq!(vsize_for(&[Legacy], &[22, 25]), 224);
        assert_eq!(vsize_for(&[Taproot, NativeSegwit], &[22, 34]), 211);
    }

   
    #[test]
    fn the_live_rates_are_four_distinct_prices() {
        let live = [0.1f32, 0.37, 0.38, 0.43].map(|r| tier_sats_at(r, 140));
        assert_eq!(live, [14, 52, 54, 61]);
       
        assert!(live[0] <= live[3], "the minimum is outbidding the top tier");
    }
}

#[cfg(test)]
mod offer_tests {
    use super::*;
    use crate::channel::BtcUtxo;

    #[test]
    fn a_marked_coin_is_never_offered_to_the_signer() {
        let wallet = "bc1qoffer-test";
        let coin = |txid: &str, sats: u64, height: u64, spent_by: Option<&str>| BtcUtxo {
            txid: txid.to_string(),
            vout: 0,
            sats,
            height,
            address: wallet.to_string(),
            spent_by: spent_by.map(String::from),
        };
        CHANNEL.btc_utxos_tx.send_replace((
            Some(wallet.to_string()),
            vec![
                coin("free", 5_923, 965_711, None),
                coin("consumed", 5_802, 969_974, Some("33cf")),
                coin("foreign", 100, 0, None),
            ],
        ));
        let offered: Vec<String> = eligible_utxos(wallet).unwrap().into_iter().map(|u| u.txid).collect();
        assert_eq!(offered, vec!["free".to_string()]);

        let info = crate::channel::BtcRbfInfo {
            txid: "bumped".to_string(),
            vsize: 141,
            fee_sats: 141,
            inputs: Vec::new(),
            outputs: Vec::new(),
            incremental_sat_vb: 1.0,
            min_sat_vb: 1.0,
            descendants: 0,
        };
        CHANNEL.btc_utxos_tx.send_replace((
            Some(wallet.to_string()),
            vec![coin("consumed", 5_802, 900_000, Some("33cf")), coin("free", 5_923, 965_711, None)],
        ));
        let spares: Vec<String> = spare_coins(wallet, &info).into_iter().map(|u| u.txid).collect();
        assert_eq!(spares, vec!["free".to_string()]);
    }
}
