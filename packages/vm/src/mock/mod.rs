use std::collections::HashMap;

use crate::{Api, Extern, FfiResult, Querier, QuerierResult};
use cosmwasm_std::{
    from_slice, to_binary, AllBalanceResponse, BalanceResponse, BankQuery, Binary, BlockInfo,
    CanonicalAddr, Coin, ContractInfo, Delegation, Env, HumanAddr, MessageInfo, Never,
    QueryRequest, SystemError, Validator, WasmQuery,
};

mod storage;

use storage::MemoryStorage;

static CONTRACT_ADDR: &str = "cosmos2contract";

/// All external requirements that can be injected for unit tests.
/// It sets the given balance for the contract itself, nothing else
pub fn mock_dependencies(
    canonical_length: usize,
    contract_balance: &[Coin],
) -> Extern<MockStorage, MockApi, MockQuerier> {
    let contract_addr = HumanAddr::from(CONTRACT_ADDR);
    Extern {
        storage: MockStorage::default(),
        api: MockApi::new(canonical_length),
        querier: MockQuerier::new(&[(&contract_addr, contract_balance)]),
    }
}

/// Initializes the querier along with the mock_dependencies.
/// Sets all balances provided (yoy must explicitly set contract balance if desired)
pub fn mock_dependencies_with_balances(
    canonical_length: usize,
    balances: &[(&HumanAddr, &[Coin])],
) -> Extern<MockStorage, MockApi, MockQuerier> {
    Extern {
        storage: MockStorage::default(),
        api: MockApi::new(canonical_length),
        querier: MockQuerier::new(balances),
    }
}

// Use MemoryStorage implementation (which is valid in non-testcode)
// We can later make simplifications here if needed
pub type MockStorage = MemoryStorage;

// MockPrecompiles zero pads all human addresses to make them fit the canonical_length
// it trims off zeros for the reverse operation.
// not really smart, but allows us to see a difference (and consistent length for canonical adddresses)
#[derive(Copy, Clone)]
pub struct MockApi {
    canonical_length: usize,
}

impl MockApi {
    pub fn new(canonical_length: usize) -> Self {
        MockApi { canonical_length }
    }
}

impl Default for MockApi {
    fn default() -> Self {
        Self::new(20)
    }
}

impl Api for MockApi {
    fn canonical_address(&self, human: &HumanAddr) -> FfiResult<CanonicalAddr> {
        // Dummy input validation. This is more sophisticated for formats like bech32, where format and checksum are validated.
        if human.len() < 3 {
            return Err(crate::make_ffi_other(
                "Invalid input: human address too short",
            ));
        }
        if human.len() > self.canonical_length {
            return Err(crate::make_ffi_other(
                "Invalid input: human address too long",
            ));
        }

        let mut out = Vec::from(human.as_str());
        let append = self.canonical_length - out.len();
        if append > 0 {
            out.extend(vec![0u8; append]);
        }
        Ok(CanonicalAddr(Binary(out)))
    }

    fn human_address(&self, canonical: &CanonicalAddr) -> FfiResult<HumanAddr> {
        if canonical.len() != self.canonical_length {
            return Err(crate::make_ffi_other(
                "Invalid input: canonical address length not correct",
            ));
        }

        // remove trailing 0's (TODO: fix this - but fine for first tests)
        let trimmed: Vec<u8> = canonical
            .as_slice()
            .iter()
            .cloned()
            .filter(|&x| x != 0)
            .collect();
        // decode UTF-8 bytes into string
        let human = String::from_utf8(trimmed)
            .map_err(|_| crate::make_ffi_other("Could not parse human address result as utf-8"))?;
        Ok(HumanAddr(human))
    }
}

/// Just set sender and sent funds for the message. The rest uses defaults.
/// The sender will be canonicalized internally to allow developers pasing in human readable senders.
/// This is intended for use in test code only.
pub fn mock_env<T: Api, U: Into<HumanAddr>>(api: &T, sender: U, sent: &[Coin]) -> Env {
    Env {
        block: BlockInfo {
            height: 12_345,
            time: 1_571_797_419,
            chain_id: "cosmos-testnet-14002".to_string(),
        },
        message: MessageInfo {
            sender: api.canonical_address(&sender.into()).unwrap(),
            sent_funds: sent.to_vec(),
        },
        contract: ContractInfo {
            address: api
                .canonical_address(&HumanAddr::from(CONTRACT_ADDR))
                .unwrap(),
        },
    }
}

/// MockQuerier holds an immutable table of bank balances
/// TODO: also allow querying contracts
#[derive(Clone, Default)]
pub struct MockQuerier {
    bank: BankQuerier,
    staking: staking::StakingQuerier,
    // placeholder to add support later
    wasm: NoWasmQuerier,
}

impl MockQuerier {
    pub fn new(balances: &[(&HumanAddr, &[Coin])]) -> Self {
        MockQuerier {
            bank: BankQuerier::new(balances),
            staking: staking::StakingQuerier::default(),
            wasm: NoWasmQuerier {},
        }
    }

    pub fn with_staking(
        &mut self,
        denom: &str,
        validators: &[Validator],
        delegations: &[Delegation],
    ) {
        self.staking = staking::StakingQuerier::new(denom, validators, delegations);
    }
}

impl Querier for MockQuerier {
    fn raw_query(&self, bin_request: &[u8]) -> QuerierResult {
        // MockQuerier doesn't support Custom, so we ignore it completely here
        let request: QueryRequest<Never> = match from_slice(bin_request) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Err(SystemError::InvalidRequest {
                    error: format!("Parsing QueryRequest: {}", e),
                    request: Binary(bin_request.to_vec()),
                }))
            }
        };
        self.handle_query(&request)
    }
}

impl MockQuerier {
    pub fn handle_query<T>(&self, request: &QueryRequest<T>) -> QuerierResult {
        match &request {
            QueryRequest::Bank(bank_query) => self.bank.query(bank_query),
            QueryRequest::Custom(_) => Ok(Err(SystemError::UnsupportedRequest {
                kind: "custom".to_string(),
            })),
            QueryRequest::Staking(staking_query) => self.staking.query(staking_query),
            QueryRequest::Wasm(msg) => self.wasm.query(msg),
        }
    }
}

#[derive(Clone, Default)]
struct NoWasmQuerier {
    // FIXME: actually provide a way to call out
}

impl NoWasmQuerier {
    fn query(&self, request: &WasmQuery) -> QuerierResult {
        let addr = match request {
            WasmQuery::Smart { contract_addr, .. } => contract_addr,
            WasmQuery::Raw { contract_addr, .. } => contract_addr,
        }
        .clone();
        Ok(Err(SystemError::NoSuchContract { addr }))
    }
}

#[derive(Clone, Default)]
struct BankQuerier {
    balances: HashMap<HumanAddr, Vec<Coin>>,
}

impl BankQuerier {
    fn new(balances: &[(&HumanAddr, &[Coin])]) -> Self {
        let mut map = HashMap::new();
        for (addr, coins) in balances.iter() {
            map.insert(HumanAddr::from(addr), coins.to_vec());
        }
        BankQuerier { balances: map }
    }

    fn query(&self, request: &BankQuery) -> QuerierResult {
        match request {
            BankQuery::Balance { address, denom } => {
                // proper error on not found, serialize result on found
                let amount = self
                    .balances
                    .get(address)
                    .and_then(|v| v.iter().find(|c| &c.denom == denom).map(|c| c.amount))
                    .unwrap_or_default();
                let bank_res = BalanceResponse {
                    amount: Coin {
                        amount,
                        denom: denom.to_string(),
                    },
                };
                Ok(Ok(to_binary(&bank_res)))
            }
            BankQuery::AllBalances { address } => {
                // proper error on not found, serialize result on found
                let bank_res = AllBalanceResponse {
                    amount: self.balances.get(address).cloned().unwrap_or_default(),
                };
                Ok(Ok(to_binary(&bank_res)))
            }
        }
    }
}

mod staking {
    use crate::traits::QuerierResult;
    use cosmwasm_std::{
        to_binary, BondedDenomResponse, Delegation, DelegationsResponse, StakingQuery, Validator,
        ValidatorsResponse,
    };

    #[derive(Clone, Default)]
    pub struct StakingQuerier {
        denom: String,
        validators: Vec<Validator>,
        delegations: Vec<Delegation>,
    }

    impl StakingQuerier {
        pub fn new(denom: &str, validators: &[Validator], delegations: &[Delegation]) -> Self {
            StakingQuerier {
                denom: denom.to_string(),
                validators: validators.to_vec(),
                delegations: delegations.to_vec(),
            }
        }

        pub fn query(&self, request: &StakingQuery) -> QuerierResult {
            match request {
                StakingQuery::BondedDenom {} => {
                    let res = BondedDenomResponse {
                        denom: self.denom.clone(),
                    };
                    Ok(Ok(to_binary(&res)))
                }
                StakingQuery::Validators {} => {
                    let res = ValidatorsResponse {
                        validators: self.validators.clone(),
                    };
                    Ok(Ok(to_binary(&res)))
                }
                StakingQuery::Delegations {
                    delegator,
                    validator,
                } => {
                    let matches = |d: &&Delegation| {
                        if let Some(val) = validator {
                            if val != &d.validator {
                                return false;
                            }
                        }
                        &d.delegator == delegator
                    };
                    let delegations: Vec<_> =
                        self.delegations.iter().filter(matches).cloned().collect();
                    let res = DelegationsResponse { delegations };
                    Ok(Ok(to_binary(&res)))
                }
            }
        }
    }

    #[cfg(test)]
    mod test {
        use super::*;

        use cosmwasm_std::{coin, from_binary, Decimal, HumanAddr};

        #[test]
        fn staking_querier_validators() {
            let val1 = Validator {
                address: HumanAddr::from("validator-one"),
                commission: Decimal::percent(1),
                max_commission: Decimal::percent(3),
                max_change_rate: Decimal::percent(1),
            };
            let val2 = Validator {
                address: HumanAddr::from("validator-two"),
                commission: Decimal::permille(15),
                max_commission: Decimal::permille(40),
                max_change_rate: Decimal::permille(5),
            };

            let staking = StakingQuerier::new("stake", &[val1.clone(), val2.clone()], &[]);

            // one match
            let raw = staking
                .query(&StakingQuery::Validators {})
                .unwrap()
                .unwrap()
                .unwrap();
            let vals: ValidatorsResponse = from_binary(&raw).unwrap();
            assert_eq!(vals.validators, vec![val1, val2]);
        }

        // gets delegators from query or panic
        fn get_delegators(
            staking: &StakingQuerier,
            delegator: HumanAddr,
            validator: Option<HumanAddr>,
        ) -> Vec<Delegation> {
            let raw = staking
                .query(&StakingQuery::Delegations {
                    delegator,
                    validator,
                })
                .unwrap()
                .unwrap()
                .unwrap();
            let dels: DelegationsResponse = from_binary(&raw).unwrap();
            dels.delegations
        }

        #[test]
        fn staking_querier_delegations() {
            let val1 = HumanAddr::from("validator-one");
            let val2 = HumanAddr::from("validator-two");

            let user_a = HumanAddr::from("investor");
            let user_b = HumanAddr::from("speculator");
            let user_c = HumanAddr::from("hodler");

            // we need multiple validators per delegator, so the queries provide different results
            let del1a = Delegation {
                delegator: user_a.clone(),
                validator: val1.clone(),
                amount: coin(100, "stake"),
                can_redelegate: true,
                accumulated_rewards: coin(5, "stake"),
            };
            let del2a = Delegation {
                delegator: user_a.clone(),
                validator: val2.clone(),
                amount: coin(500, "stake"),
                can_redelegate: true,
                accumulated_rewards: coin(20, "stake"),
            };

            // this is multiple times on the same validator
            let del1b = Delegation {
                delegator: user_b.clone(),
                validator: val1.clone(),
                amount: coin(500, "stake"),
                can_redelegate: false,
                accumulated_rewards: coin(0, "stake"),
            };
            let del1bb = Delegation {
                delegator: user_b.clone(),
                validator: val1.clone(),
                amount: coin(700, "stake"),
                can_redelegate: true,
                accumulated_rewards: coin(70, "stake"),
            };

            // and another one on val2
            let del2c = Delegation {
                delegator: user_c.clone(),
                validator: val2.clone(),
                amount: coin(8888, "stake"),
                can_redelegate: true,
                accumulated_rewards: coin(900, "stake"),
            };

            let staking = StakingQuerier::new(
                "stake",
                &[],
                &[
                    del1a.clone(),
                    del1b.clone(),
                    del1bb.clone(),
                    del2a.clone(),
                    del2c.clone(),
                ],
            );

            // get all for user a
            let dels = get_delegators(&staking, user_a.clone(), None);
            assert_eq!(dels, vec![del1a.clone(), del2a.clone()]);

            // get all for user b
            let dels = get_delegators(&staking, user_b.clone(), None);
            assert_eq!(dels, vec![del1b.clone(), del1bb.clone()]);

            // get all for user c
            let dels = get_delegators(&staking, user_c.clone(), None);
            assert_eq!(dels, vec![del2c.clone()]);

            // for user with no delegations...
            let dels = get_delegators(&staking, HumanAddr::from("no one"), None);
            assert_eq!(dels, vec![]);

            // filter a by validator (1 and 1)
            let dels = get_delegators(&staking, user_a.clone(), Some(val1.clone()));
            assert_eq!(dels, vec![del1a.clone()]);
            let dels = get_delegators(&staking, user_a.clone(), Some(val2.clone()));
            assert_eq!(dels, vec![del2a.clone()]);

            // filter b by validator (2 and 0)
            let dels = get_delegators(&staking, user_b.clone(), Some(val1.clone()));
            assert_eq!(dels, vec![del1b.clone(), del1bb.clone()]);
            let dels = get_delegators(&staking, user_b.clone(), Some(val2.clone()));
            assert_eq!(dels, vec![]);

            // filter c by validator (0 and 1)
            let dels = get_delegators(&staking, user_c.clone(), Some(val1.clone()));
            assert_eq!(dels, vec![]);
            let dels = get_delegators(&staking, user_c.clone(), Some(val2.clone()));
            assert_eq!(dels, vec![del2c.clone()]);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::FfiError;
    use cosmwasm_std::{coin, coins, from_binary};

    #[test]
    fn mock_env_arguments() {
        let name = HumanAddr("my name".to_string());
        let api = MockApi::new(20);

        // make sure we can generate with &str, &HumanAddr, and HumanAddr
        let a = mock_env(&api, "my name", &coins(100, "atom"));
        let b = mock_env(&api, &name, &coins(100, "atom"));
        let c = mock_env(&api, name, &coins(100, "atom"));

        // and the results are the same
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn flip_addresses() {
        let api = MockApi::new(20);
        let human = HumanAddr("shorty".to_string());
        let canon = api.canonical_address(&human).unwrap();
        assert_eq!(canon.len(), 20);
        assert_eq!(&canon.as_slice()[0..6], human.as_str().as_bytes());
        assert_eq!(&canon.as_slice()[6..], &[0u8; 14]);

        let recovered = api.human_address(&canon).unwrap();
        assert_eq!(human, recovered);
    }

    #[test]
    fn human_address_input_length() {
        let api = MockApi::new(10);
        let input = CanonicalAddr(Binary(vec![61; 11]));
        match api.human_address(&input).unwrap_err() {
            FfiError::Other { .. } => {}
            err => panic!("Unexpected error: {}", err),
        }
    }

    #[test]
    fn canonical_address_min_input_length() {
        let api = MockApi::new(10);
        let human = HumanAddr("1".to_string());
        match api.canonical_address(&human).unwrap_err() {
            FfiError::Other { .. } => {}
            err => panic!("Unexpected error: {}", err),
        }
    }

    #[test]
    fn canonical_address_max_input_length() {
        let api = MockApi::new(10);
        let human = HumanAddr("longer-than-10".to_string());
        match api.canonical_address(&human).unwrap_err() {
            FfiError::Other { .. } => {}
            err => panic!("Unexpected error: {}", err),
        }
    }

    #[test]
    fn bank_querier_all_balances() {
        let addr = HumanAddr::from("foobar");
        let balance = vec![coin(123, "ELF"), coin(777, "FLY")];
        let bank = BankQuerier::new(&[(&addr, &balance)]);

        // all
        let all = bank
            .query(&BankQuery::AllBalances {
                address: addr.clone(),
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let res: AllBalanceResponse = from_binary(&all).unwrap();
        assert_eq!(&res.amount, &balance);
    }

    #[test]
    fn bank_querier_one_balance() {
        let addr = HumanAddr::from("foobar");
        let balance = vec![coin(123, "ELF"), coin(777, "FLY")];
        let bank = BankQuerier::new(&[(&addr, &balance)]);

        // one match
        let fly = bank
            .query(&BankQuery::Balance {
                address: addr.clone(),
                denom: "FLY".to_string(),
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let res: BalanceResponse = from_binary(&fly).unwrap();
        assert_eq!(res.amount, coin(777, "FLY"));

        // missing denom
        let miss = bank
            .query(&BankQuery::Balance {
                address: addr.clone(),
                denom: "MISS".to_string(),
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let res: BalanceResponse = from_binary(&miss).unwrap();
        assert_eq!(res.amount, coin(0, "MISS"));
    }

    #[test]
    fn bank_querier_missing_account() {
        let addr = HumanAddr::from("foobar");
        let balance = vec![coin(123, "ELF"), coin(777, "FLY")];
        let bank = BankQuerier::new(&[(&addr, &balance)]);

        // all balances on empty account is empty vec
        let all = bank
            .query(&BankQuery::AllBalances {
                address: HumanAddr::from("elsewhere"),
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let res: AllBalanceResponse = from_binary(&all).unwrap();
        assert_eq!(res.amount, vec![]);

        // any denom on balances on empty account is empty coin
        let miss = bank
            .query(&BankQuery::Balance {
                address: HumanAddr::from("elsewhere"),
                denom: "ELF".to_string(),
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let res: BalanceResponse = from_binary(&miss).unwrap();
        assert_eq!(res.amount, coin(0, "ELF"));
    }
}