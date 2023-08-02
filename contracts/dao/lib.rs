#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[ink::contract]
pub mod dao {
    use ink::env::call::{build_call, ExecutionInput, Selector};
    use ink::env::DefaultEnvironment;
    use ink::storage::Mapping;
    use openbrush::contracts::traits::psp22::*;
    use scale::{
        Decode,
        Encode,
    };

    #[derive(Encode, Decode)]
    #[cfg_attr(feature = "std", derive(Debug, PartialEq, Eq, scale_info::TypeInfo))]
    pub enum VoteType {
        Against,
        For,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq, Encode, Decode)]
    #[cfg_attr(feature = "std", derive(scale_info::TypeInfo))]
    pub enum GovernorError {
        ProposalNotFound,
        ProposalAlreadyExecuted,
        QuorumNotReached,
        ProposalNotAccepted,
        AmountShouldNotBeZero,
        DurationError,
        VotePeriodEnded,
        AlreadyVoted,
        TxFailed,
    }

    #[derive(Encode, Decode)]
    #[cfg_attr(
        feature = "std",
        derive(
            Debug,
            PartialEq,
            Eq,
            scale_info::TypeInfo,
            ink::storage::traits::StorageLayout
        )
    )]
    pub struct Proposal {
        to: AccountId,
        vote_start: u64,
        vote_end: u64,
        executed: bool,
        amount: Balance,
    }

    #[derive(Encode, Decode, Default)]
    #[cfg_attr(
        feature = "std",
        derive(
            Debug,
            PartialEq,
            Eq,
            scale_info::TypeInfo,
            ink::storage::traits::StorageLayout
        )
    )]
    pub struct ProposalVote {
        for_votes: u128,
        against_votes: u128,
    }

    const ONE_MINUTE: u64 = 60;
    pub type ProposalId = u64;

    #[ink(storage)]
    pub struct Governor {
        proposals: Mapping<ProposalId, Proposal>,
        proposal_votes: Mapping<Proposal, ProposalVote>,
        votes: Mapping<(ProposalId, AccountId), ()>,
        next_proposal_id: ProposalId,
        quorum: u8,
        governance_token: AccountId,
    }

    impl Governor {
        #[ink(constructor, payable)]
        pub fn new(governance_token: AccountId, quorum: u8) -> Self {
            Governor {
                proposals: Default::default(),
                proposal_votes: Default::default(),
                votes: Default::default(),
                next_proposal_id: Default::default(),
                governance_token,
                quorum,
            }
        }

        #[ink(message)]
        pub fn propose(
            &mut self,
            to: AccountId,
            amount: Balance,
            duration: u64,
        ) -> Result<(), GovernorError> {
            if amount <= 0 {
                return Err(GovernorError::AmountShouldNotBeZero)
            }

            if duration <= 0 {
                return Err(GovernorError::DurationError)
            }

            let proposal = Proposal {
                to,
                vote_start: self.env().block_timestamp(),
                vote_end: self.env().block_timestamp() + duration * ONE_MINUTE,
                executed: false,
                amount,
            };

            self.next_proposal_id = self.next_proposal_id() + 1;
            self.proposals.insert(self.next_proposal_id, &proposal);
            self.proposal_votes.insert(proposal, &{ProposalVote {
                for_votes: 0,
                against_votes: 0
            }});

            Ok(())
        }

        #[ink(message)]
        pub fn vote(
            &mut self,
            proposal_id: ProposalId,
            vote: VoteType,
        ) -> Result<(), GovernorError> {
            if self.proposals.contains(&proposal_id) {
                return Err(GovernorError::ProposalNotFound)
            };

            match self.get_proposal(proposal_id.clone()) {
                None => {}
                Some(p) => {
                    if p.executed == true {
                        return Err(GovernorError::ProposalAlreadyExecuted)
                    }

                    if p.vote_end < self.env().block_timestamp() {
                        return Err(GovernorError::VotePeriodEnded)
                    }
                }
            }

            let caller = self.env().caller();

            if self.votes.contains(&(proposal_id, caller)) {
                return Err(GovernorError::AlreadyVoted)
            }

            self.votes.insert(&(proposal_id, caller), &());

            let caller_balance = self.balance_of_acc(caller);
            let total_balance = self.get_total_supply();

            let weight = caller_balance / total_balance * 100;

            let p = self.get_proposal(proposal_id).unwrap();

            let mut votes = self.proposal_votes.get(&p).expect("not found");

            match vote {
                VoteType::Against => {
                    votes.against_votes += weight;
                }
                VoteType::For => {
                    votes.for_votes += weight;
                }
            };

            self.proposal_votes.insert(p, &votes);

            Ok(())
        }

        #[ink(message)]
        pub fn execute(&mut self, proposal_id: ProposalId) -> Result<(), GovernorError> {
            if self.proposals.contains(&proposal_id) {
                return Err(GovernorError::ProposalNotFound);
            };

            let mut p = self.get_proposal(proposal_id).unwrap();

            if p.executed == true {
                return Err(GovernorError::ProposalAlreadyExecuted)
            }

            if let Some(votes) = self.get_proposal_votes(proposal_id) {
                if votes.against_votes + votes.for_votes < self.quorum.into() {
                    return Err(GovernorError::QuorumNotReached);
                }

                if votes.against_votes < votes.for_votes {
                    return Err(GovernorError::ProposalNotAccepted);
                }
            }

            p.executed = true;

            build_call::<DefaultEnvironment>()
                .call(self.governance_token)
                .gas_limit(5_000_000_000)
                .exec_input(
                    ExecutionInput::new(Selector::new(ink::selector_bytes!(
                        "PSP22::transfer"
                    )))
                        .push_arg(p.to)
                        .push_arg(p.amount),
                )
                .returns::<()>()
                .try_invoke()
                .map_err(|_| GovernorError::TxFailed)?
                .map_err(|_| GovernorError::TxFailed)?;

            Ok(())
        }

        #[ink(message)]
        pub fn get_proposal(&self, proposal_id: ProposalId) -> Option<Proposal> {
            if let Some(p) = self.proposals.get(proposal_id) {
                Some(p)
            } else {
                None
            }
        }

        #[ink(message)]
        pub fn next_proposal_id(&self) -> ProposalId  {
            self.next_proposal_id
        }

        fn get_proposal_votes(&self, proposal_id: ProposalId) -> Option<ProposalVote> {
            let p = self.get_proposal(proposal_id).unwrap();
            if let Some(votes_distribution) = self.proposal_votes.get(&p) {
                Some(votes_distribution)
            } else {
                None
            }
        }

        fn balance_of_acc(&self, account_id: AccountId) -> Balance {
            build_call::<DefaultEnvironment>()
                .call(self.governance_token)
                .gas_limit(0)
                .exec_input(
                    ExecutionInput::new(Selector::new(ink::selector_bytes!("balance_of")))
                        .push_arg(&account_id)
                )
                .returns::<Balance>()
                .invoke()
        }

        fn get_total_supply(&self) -> Balance {
            build_call::<DefaultEnvironment>()
                .call(self.governance_token)
                .gas_limit(0)
                .exec_input(
                    ExecutionInput::new(Selector::new(ink::selector_bytes!("total_supply")))
                )
                .returns::<Balance>()
                .invoke()
        }

        // used for test
        #[ink(message)]
        pub fn now(&self) -> u64 {
            self.env().block_timestamp()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn create_contract(initial_balance: Balance) -> Governor {
            let accounts = default_accounts();
            set_sender(accounts.alice);
            set_balance(contract_id(), initial_balance);
            Governor::new(AccountId::from([0x01; 32]), 50)
        }

        fn contract_id() -> AccountId {
            ink::env::test::callee::<ink::env::DefaultEnvironment>()
        }

        fn default_accounts(
        ) -> ink::env::test::DefaultAccounts<ink::env::DefaultEnvironment> {
            ink::env::test::default_accounts::<ink::env::DefaultEnvironment>()
        }

        fn set_sender(sender: AccountId) {
            ink::env::test::set_caller::<ink::env::DefaultEnvironment>(sender);
        }

        fn set_balance(account_id: AccountId, balance: Balance) {
            ink::env::test::set_account_balance::<ink::env::DefaultEnvironment>(
                account_id, balance,
            )
        }

        #[ink::test]
        fn propose_works() {
            let accounts = default_accounts();
            let mut governor = create_contract(1000);
            assert_eq!(
                governor.propose(accounts.django, 0, 1),
                Err(GovernorError::AmountShouldNotBeZero)
            );
            assert_eq!(
                governor.propose(accounts.django, 100, 0),
                Err(GovernorError::DurationError)
            );
            let result = governor.propose(accounts.django, 100, 1);
            assert_eq!(result, Ok(()));
            let proposal = governor.get_proposal(1).unwrap();
            let now = governor.now();
            assert_eq!(
                proposal,
                Proposal {
                    to: accounts.django,
                    amount: 100,
                    vote_start: 0,
                    vote_end: now + 1 * ONE_MINUTE,
                    executed: false,
                }
            );
            assert_eq!(governor.next_proposal_id(), 1);
        }

        #[ink::test]
        fn quorum_not_reached() {
            let mut governor = create_contract(1000);
            let result = governor.propose(AccountId::from([0x02; 32]), 100, 1);
            assert_eq!(result, Ok(()));
            assert_eq!(governor.next_proposal_id(), 1);
            let execute = governor.execute(1);
            assert_eq!(execute, Err(GovernorError::ProposalNotFound));
        }
    }
}
