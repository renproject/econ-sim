type USD = f64;
type Percentage = f64;

//
// STATE
// For capturing the state of RenVM throughout the simulation.
//

/// State represents the state of RenVM at the end of an epoch. All values in the state are derived
/// from the behaviour of the external and internal models; they are never directly simulated. If
/// you find yourself directly modifying the state, you are probably doing something wrong.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    tvb: USD,
    tvl: USD,
    tvr: USD,

    mf: Percentage,
    bf: Percentage,
    r: Percentage,

    f_unclaimed: USD,
    f_claimed: USD,
    r_pool: USD,
}

//
// EXTERNAL
// For modelling the behaviour of entities that are external to RenVM. This means that the entity
// is capable of taking action independently of the precise mechanics of RenVM. For example,
// minters, burners, and node operators should all be considered external. Modify these functions
// when you want to see how the state of RenVM will evolve under different assumptions about how
// people will behave. For example, you can modify `total_value_bonded` to model different node
// operator (dis)bonding behaviour.
//

/// This function returns the amount of USD that is bonded to RenVM. Changing this function allows
/// you to model the behaviour of node operators.
fn total_value_bonded(history: &Vec<State>) -> USD {
    // The basic model assumes that node operators want to receive some target ROI based on the one
    // week average fee. 
    let per_annum = history.windows(2)
        .rev()
        .take(7)
        .map(|w| w[1].f_claimed - w[0].f_claimed)
        .sum::<f64>() / 7.0 * 365.0;
    let roi = 0.05;
    per_annum / roi
}

/// This function returns the amount of value in USD that will be minted. There are lots of factors
/// to consider here: growth of the network, historical minting fees, random deviation, etc. so it
/// is important to test different models (both rational and irrational).
fn mint_volume(history: &Vec<State>) -> USD {
    // The basic model assumes that there will be ~$4M minted per epoch (unaffected by the minting
    // fee, which is obviously unrealistic).
    4_000_000.0
}

/// This function is the same as the `mint_volume` function, but for burning volume. 
fn burn_volume(history: &Vec<State>) -> USD {
    // // The basic model assumes that there will be ~$2M burned per epoch (unaffected by the burning
    // // fee, or the rebate, which is obviously unrealistic).
    // 2_000_000.0
    
    // A more complex model considers the available rebate, and adjusts volume accordingly. In this
    // model, it is assumed that 0.1% is sufficiently high to incentivise arbitrage of up to $1M
    // per 0.1% rebate (which also means that at least $1K must be available in the rebate pool.
    let state = latest_state(history);
    if state.r >= 0.001 {
        // Consider the rebate fee.
        2_000_000.0 + (state.r_pool / state.r).min(1_000_000.0 * (state.r / 0.001))
    } else {
        // Default to the basic model.
        2_000_000.0
    }
}

//
// INTERNAL
// For modelling the behaviour of components internal to RenVM. These are components that can be
// defined and constrained by the actual implementation of RenVM, and so there is no uncertainty in
// the way that the component will behave (unlike when thinking about external entities). Modify
// these function when you want to test different designs for RenVM. For example, you could modify
// `rebate_curve` to always return zero if you want to see how the state of RenVM evolves over time
// when there are no rebates available.
//

/// This function returns the minting fee given the current state (and history) of RenVM. For
/// example, you could design a model such that minting fees rise slowly if minting volume is
/// rising (and vice versa).
fn mint_fee_curve(history: &Vec<State>) -> Percentage {
    // In production, RenVM began with a simple (and static) 0.1% minting fee.
    0.003
}

/// This function is the same as the `mint_fee_curve` function, but for burning fees. An important
/// difference is that burning fees *must* be zero when the rebate is non-zero.
fn burn_fee_curve(history: &Vec<State>) -> Percentage {
    let state = latest_state(history);
    if state.tvl < state.tvb {
        // In production, RenVM began with a simple (and static) 0.1% minting fee.
        0.001
    } else {
        0.0
    }
}

/// This function models the rebate that will be paid (as a percentage) when burning happens.
/// Whenever this value is non-zero, the `burn_fee_curve` function *must* return zero (it makes no
/// sense to offer a rebate in the presence of a burning fee; the better thing to do would be to
/// remove the burning fee, which has the same initial effect).
fn rebate_curve(history: &Vec<State>) -> Percentage {
    let state = latest_state(history);
    if state.tvb < state.tvl {
        // If TVL-TVB has decreased in the last epoch compared to the weekly average, then slowly
        // decrease the rebate. Otherwise, slowly increase the rebate.
        if state.tvl-state.tvb < history.iter().rev().take(7).map(|state| state.tvl-state.tvb).sum::<f64>() / 7.0 {
            (state.r - 0.0001).max(0.0)
        } else {
            state.r + 0.0001
        }
    } else {
        0.0
    }
}

/// This function returns the amount of fees that are going to be made available for rebating. Fees
/// that are made available for rebating are *not* paid to the nodes (this is already taken into
/// consideration; `State::f` and `State::f_claimed` will not include fees that have been made
/// available for rebating).
fn rebate_collected(history: &Vec<State>, f: USD) -> USD {
    // 50% of fees are made available as a rebate.
    f * 0.5
}

//
// MAIN
// For running the simulation. You probably do not need to modify this code at all.
//

fn main() {
    println!("initialising...");

    let num_steps = 180; 

    drop((0..num_steps).fold(vec!(State::default()), |mut history, step| {
        let mut state = latest_state(&history);

        // Mint and burn volumes this epoch.
        let mv = mint_volume(&history);
        let bv = burn_volume(&history);

        // Fees and rebate collected this epoch.
        let mf = mint_fee_curve(&history);
        let bf = burn_fee_curve(&history); 
        let r = rebate_curve(&history);
        let r_paid = bv*r;
        let f_collected = mv*mf + bv*bf;
        let r_collected = rebate_collected(&history, f_collected);
        let f_collected = f_collected - r_collected;

        // Update the total values bonded, locked, and available for rebate
        state.tvb = total_value_bonded(&history);
        state.tvl += mv - bv;
        state.tvr += r_collected;
        
        // Update the fee and rebate curves
        state.mf = mf;
        state.bf = bf;
        state.r = r;
        
        // Update the fees claimed by nodes and the fees collected in total (including all of the
        // fees claimed up until this point)
        let claim = state.f_unclaimed*0.024451;
        state.f_unclaimed += f_collected - claim;
        state.f_claimed += claim; // Claim ~2% of available fees per epoch (~50% per month)
        state.r_pool = (state.r_pool + r_collected - r_paid).max(0.0);
        println!(
            "[{}] tvl={:.2} tvb={:.2} f_claimed={:.2} r_pool={:.2}",
            step, 
            state.tvl,
            state.tvb,
            state.f_claimed,
            state.r_pool,
        );

        history.push(state);
        history
    }));

    println!("done");
}

/// Helper function to get a copy of the latest state from a history of states.
fn latest_state(history: &Vec<State>) -> State {
    history.iter().last().expect("missing initial state").clone()
}
