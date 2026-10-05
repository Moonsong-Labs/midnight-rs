(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (block_time_gt . %block_time_gt.4)
    (block_time_lt . %block_time_lt.5)
    (checkpoint_now . %checkpoint_now.2)
    (claim_nullifier . %claim_nullifier.3) (mint . %mint.0)
    (unshielded_balance_gt . %unshielded_balance_gt.1))
  (contract-types)
  (kernel-declaration (%kernel.11 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %left.18 (exported #f) (pure #t) (proof #f)
    ((%value.22 (tbytes 32)))
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
           (var-ref %value.22)
           (default (tbytes 32)))))
  (circuit %blockTimeLt.6 (exported #f) (pure #f) (proof #f)
    ((%time.23 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.11 read () blockTimeLessThan (tboolean)
        (instructions (dup (n 2))
          (idx (cached #t) (pushPath #f) (path ((align 2 1))))
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.23))))
          (lt) (popeq (cached #t) (result (void))))
        (var-ref %time.23))))
  (circuit %blockTimeGt.14 (exported #f) (pure #f) (proof #f)
    ((%time.17 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.11 read () blockTimeGreaterThan (tboolean)
        (instructions
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.17))))
          (dup (n 3))
          (idx (cached #t) (pushPath #f) (path ((align 2 1)))) (lt)
          (popeq (cached #t) (result (void))))
        (var-ref %time.17))))
  (circuit %unshieldedBalanceGt.8 (exported #f) (pure #f) (proof #f)
    ((%color.19 (tbytes 32))
      (%amount.21
        (tunsigned 340282366920938463463374607431768211455)))
    (tboolean)
    (return
      (let* (((%tmp.20
                (tstruct
                  Either
                  (is_left (tboolean))
                  (left (tbytes 32))
                  (right (tbytes 32)))) (call
                                          %left.18
                                          (var-ref %color.19))))
        (public-ledger %kernel.11 read () balanceGreaterThan (tboolean)
          (instructions
            (push
              (storage #f)
              (value (state-value cell (var-ref %amount.21))))
            (dup (n 3))
            (idx (cached #t) (pushPath #f) (path ((align 5 1))))
            (dup (n 0))
            (push
              (storage #f)
              (value (state-value cell (var-ref %tmp.20))))
            (member) (branch (skip 3)) (pop)
            (push (storage #f) (value (state-value cell (align 0 16))))
            (jmp (skip 1))
            (idx (cached #t) (pushPath #f) (path ((var-ref %tmp.20))))
            (lt) (popeq (cached #t) (result (void))))
          (var-ref %tmp.20) (var-ref %amount.21)))))
  (circuit %checkpoint_now.2 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (public-ledger %kernel.11 update () checkpoint (ttuple)
           (instructions (ckpt)))
         (return (tuple))))
  (circuit %claim_nullifier.3 (exported #t) (pure #f) (proof #t)
    ((%nul.16 (tbytes 32))) (ttuple)
    (seq (public-ledger %kernel.11 update () claimZswapNullifier (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %nul.16))))
             (push (storage #f) (value (state-value null)))
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %nul.16))
         (return (tuple))))
  (circuit %mint.0 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.12 (tbytes 32))
      (%amount.13 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (public-ledger %kernel.11 update () mintShielded (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 4 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %domain_sep.12))))
             (dup (n 1)) (dup (n 1)) (member)
             (push
               (storage #f)
               (value (state-value cell (var-ref %amount.13))))
             (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
             (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %domain_sep.12) (var-ref %amount.13))
         (return (tuple))))
  (circuit %block_time_gt.4 (exported #t) (pure #f) (proof #t)
    ((%time.15 (tunsigned 18446744073709551615))) (tboolean)
    (return (call %blockTimeGt.14 (var-ref %time.15))))
  (circuit %block_time_lt.5 (exported #t) (pure #f) (proof #t)
    ((%time.7 (tunsigned 18446744073709551615))) (tboolean)
    (return (call %blockTimeLt.6 (var-ref %time.7))))
  (circuit %unshielded_balance_gt.1 (exported #t) (pure #f) (proof #t)
    ((%color.9 (tbytes 32))
      (%amount.10
        (tunsigned 340282366920938463463374607431768211455)))
    (tboolean)
    (return
      (call
        %unshieldedBalanceGt.8
        (var-ref %color.9)
        (var-ref %amount.10)))))
