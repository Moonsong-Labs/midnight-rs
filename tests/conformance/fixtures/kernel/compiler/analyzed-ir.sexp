(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (block_time_gt . %block_time_gt.5)
    (block_time_lt . %block_time_lt.6)
    (caller_coin_public_key . %caller_coin_public_key.3)
    (checkpoint_now . %checkpoint_now.4)
    (claim_nullifier . %claim_nullifier.1) (mint . %mint.2)
    (unshielded_balance_gt . %unshielded_balance_gt.0))
  (contract-types)
  (kernel-declaration (%kernel.15 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %left.20 (exported #f) (pure #t) (proof #f)
    ((%value.24 (tbytes 32)))
    (tstruct
      Either
      (is_left (tboolean))
      (left (tbytes 32))
      (right (tbytes 32)))
    (return
      (new (tstruct
             Either
             (is_left (tboolean))
             (left (tbytes 32))
             (right (tbytes 32)))
           '#t
           (var-ref %value.24)
           (default (tbytes 32)))))
  (circuit %blockTimeLt.13 (exported #f) (pure #f) (proof #f)
    ((%time.25 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.15 read () blockTimeLessThan (tboolean)
        (instructions (dup (n 2))
          (idx (cached #t) (pushPath #f) (path ((align 2 1))))
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.25))))
          (lt) (popeq (cached #t) (result (void))))
        (var-ref %time.25))))
  (circuit %blockTimeGt.11 (exported #f) (pure #f) (proof #f)
    ((%time.19 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.15 read () blockTimeGreaterThan (tboolean)
        (instructions
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.19))))
          (dup (n 3))
          (idx (cached #t) (pushPath #f) (path ((align 2 1)))) (lt)
          (popeq (cached #t) (result (void))))
        (var-ref %time.19))))
  (circuit %unshieldedBalanceGt.7 (exported #f) (pure #f) (proof #f)
    ((%color.21 (tbytes 32))
      (%amount.23
        (tunsigned 340282366920938463463374607431768211455)))
    (tboolean)
    (return
      (let* (((%tmp.22
                (tstruct
                  Either
                  (is_left (tboolean))
                  (left (tbytes 32))
                  (right (tbytes 32)))) (call
                                          %left.20
                                          (var-ref %color.21))))
        (public-ledger %kernel.15 read () balanceGreaterThan (tboolean)
          (instructions
            (push
              (storage #f)
              (value (state-value cell (var-ref %amount.23))))
            (dup (n 3))
            (idx (cached #t) (pushPath #f) (path ((align 5 1))))
            (dup (n 0))
            (push
              (storage #f)
              (value (state-value cell (var-ref %tmp.22))))
            (member) (branch (skip 3)) (pop)
            (push (storage #f) (value (state-value cell (align 0 16))))
            (jmp (skip 1))
            (idx (cached #t) (pushPath #f) (path ((var-ref %tmp.22))))
            (lt) (popeq (cached #t) (result (void))))
          (var-ref %tmp.22) (var-ref %amount.23)))))
  (native %ownPublicKey.10
    (entry "__compactRuntime.ownPublicKey" witness)
    (type-arguments) ()
    (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
  (circuit %checkpoint_now.4 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (public-ledger %kernel.15 update () checkpoint (ttuple)
           (instructions (ckpt)))
         (return (tuple))))
  (circuit %claim_nullifier.1 (exported #t) (pure #f) (proof #t)
    ((%nul.16 (tbytes 32))) (ttuple)
    (seq (public-ledger %kernel.15 update () claimZswapNullifier (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %nul.16))))
             (push (storage #f) (value (state-value null)))
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %nul.16))
         (return (tuple))))
  (circuit %mint.2 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.17 (tbytes 32))
      (%amount.18 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (public-ledger %kernel.15 update () mintShielded (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 4 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %domain_sep.17))))
             (dup (n 1)) (dup (n 1)) (member)
             (push
               (storage #f)
               (value (state-value cell (var-ref %amount.18))))
             (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
             (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %domain_sep.17) (var-ref %amount.18))
         (return (tuple))))
  (circuit %block_time_gt.5 (exported #t) (pure #f) (proof #t)
    ((%time.12 (tunsigned 18446744073709551615))) (tboolean)
    (return (call %blockTimeGt.11 (var-ref %time.12))))
  (circuit %block_time_lt.6 (exported #t) (pure #f) (proof #t)
    ((%time.14 (tunsigned 18446744073709551615))) (tboolean)
    (return (call %blockTimeLt.13 (var-ref %time.14))))
  (circuit %unshielded_balance_gt.0 (exported #t) (pure #f) (proof #t)
    ((%color.8 (tbytes 32))
      (%amount.9
        (tunsigned 340282366920938463463374607431768211455)))
    (tboolean)
    (return
      (call
        %unshieldedBalanceGt.7
        (var-ref %color.8)
        (var-ref %amount.9))))
  (circuit %caller_coin_public_key.3 (exported #t) (pure #f)
    (proof #f) ()
    (tstruct ZswapCoinPublicKey (bytes (tbytes 32)))
    (return (call %ownPublicKey.10))))
