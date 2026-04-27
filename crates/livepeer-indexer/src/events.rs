//! Event types via alloy `sol!` macro JSON-path mode. Generates Rust types for every
//! event in each vendored ABI; we use a small subset (the v1 catalog from SPEC §6).

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    BondingManager,
    "../../abi/BondingManager.json"
);

sol!(
    #[allow(missing_docs)]
    TicketBroker,
    "../../abi/TicketBroker.json"
);

sol!(
    #[allow(missing_docs)]
    LivepeerToken,
    "../../abi/LivepeerToken.json"
);

sol!(
    #[allow(missing_docs)]
    RoundsManager,
    "../../abi/RoundsManager.json"
);
