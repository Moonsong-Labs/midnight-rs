(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (holds . %holds.12)
    (mint . %mint.13)
    (unlock . %unlock.10)
    (unlocked . %unlocked.11))
  (contract-types)
  (kernel-declaration (%kernel.24 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%unlocked.11 (0) (exported #t) (Counter)))
    (constructor () (tuple)))
  (circuit %left.23 (exported #f) (pure #t) (proof #f)
    ((%value.45 (tstruct ContractAddress (bytes (tbytes 32)))))
    (tstruct
      Either
      (is_left (tboolean))
      (left (tstruct ContractAddress (bytes (tbytes 32))))
      (right (tstruct UserAddress (bytes (tbytes 32)))))
    (return
      (new (tstruct
             Either
             (is_left (tboolean))
             (left (tstruct ContractAddress (bytes (tbytes 32))))
             (right (tstruct UserAddress (bytes (tbytes 32)))))
           '#t
           (var-ref %value.45)
           (default (tstruct UserAddress (bytes (tbytes 32)))))))
  (circuit %left.27 (exported #f) (pure #t) (proof #f)
    ((%value.46 (tbytes 32)))
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
           (var-ref %value.46)
           (default (tbytes 32)))))
  (circuit %tokenType.33 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.42 (tbytes 32))
      (%contractAddress.43
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.31
        (tuple
          (single (var-ref %domain_sep.42))
          (single (elt-ref (var-ref %contractAddress.43) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %blockTimeLt.20 (exported #f) (pure #f) (proof #f)
    ((%time.44 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.24 read () blockTimeLessThan (tboolean)
        (instructions (dup (n 2))
          (idx (cached #t) (pushPath #f) (path ((align 2 1))))
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.44))))
          (lt) (popeq (cached #t) (result (void))))
        (var-ref %time.44))))
  (circuit %blockTimeGt.18 (exported #f) (pure #f) (proof #f)
    ((%time.32 (tunsigned 18446744073709551615))) (tboolean)
    (return
      (public-ledger %kernel.24 read () blockTimeGreaterThan (tboolean)
        (instructions
          (push
            (storage #f)
            (value (state-value cell (var-ref %time.32))))
          (dup (n 3))
          (idx (cached #t) (pushPath #f) (path ((align 2 1)))) (lt)
          (popeq (cached #t) (result (void))))
        (var-ref %time.32))))
  (circuit %mintUnshieldedToken.22 (exported #f) (pure #f) (proof #f)
    ((%domainSep.34 (tbytes 32))
      (%amount.36 (tunsigned 18446744073709551615))
      (%recipient.39
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ContractAddress (bytes (tbytes 32))))
          (right (tstruct UserAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (seq (public-ledger %kernel.24 update () mintUnshielded (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 5 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %domainSep.34))))
             (dup (n 1)) (dup (n 1)) (member)
             (push
               (storage #f)
               (value (state-value cell (var-ref %amount.36))))
             (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
             (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %domainSep.34) (var-ref %amount.36))
         (let* (((%color.35 (tbytes 32)) (call
                                           %tokenType.33
                                           (var-ref %domainSep.34)
                                           (public-ledger %kernel.24 read () self
                                             (tstruct
                                               ContractAddress
                                               (bytes (tbytes 32)))
                                             (instructions
                                               (dup (n 2))
                                               (idx (cached #t)
                                                    (pushPath #f)
                                                    (path ((align 0 1))))
                                               (popeq
                                                 (cached #t)
                                                 (result (void))))))))
           (seq (let* (((%tmp.38
                          (tstruct
                            Either
                            (is_left (tboolean))
                            (left (tbytes 32))
                            (right (tbytes 32)))) (call
                                                    %left.27
                                                    (var-ref %color.35))))
                  (let* (((%tmp.37
                            (tunsigned
                              340282366920938463463374607431768211455)) (safe-cast
                                                                          (tunsigned
                                                                            340282366920938463463374607431768211455)
                                                                          (tunsigned
                                                                            18446744073709551615)
                                                                          (var-ref
                                                                            %amount.36))))
                    (public-ledger %kernel.24 update () claimUnshieldedCoinSpend
                      (ttuple)
                      (instructions (swap (n 0))
                        (idx (cached #t)
                             (pushPath #t)
                             (path ((align 8 1))))
                        (push
                          (storage #f)
                          (value
                            (state-value
                              cell
                              (aligned-concat
                                (var-ref %tmp.38)
                                (var-ref %recipient.39)))))
                        (dup (n 1)) (dup (n 1)) (member)
                        (push
                          (storage #f)
                          (value (state-value cell (var-ref %tmp.37))))
                        (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2))
                        (dup (n 2))
                        (idx (cached #t) (pushPath #f) (path ((stack))))
                        (add) (ins (cached #t) (n 2)) (swap (n 0)))
                      (var-ref %tmp.38) (var-ref %recipient.39)
                      (var-ref %tmp.37))))
                (if (if (elt-ref (var-ref %recipient.39) is_left 0)
                        (== (tbytes 32)
                            (elt-ref
                              (elt-ref (var-ref %recipient.39) left 1)
                              bytes
                              0)
                            (elt-ref
                              (public-ledger %kernel.24 read () self
                                (tstruct
                                  ContractAddress
                                  (bytes (tbytes 32)))
                                (instructions
                                  (dup (n 2))
                                  (idx (cached #t)
                                       (pushPath #f)
                                       (path ((align 0 1))))
                                  (popeq (cached #t) (result (void)))))
                              bytes
                              0))
                        '#f)
                    (let* (((%tmp.40
                              (tstruct
                                Either
                                (is_left (tboolean))
                                (left (tbytes 32))
                                (right (tbytes 32)))) (call
                                                        %left.27
                                                        (var-ref
                                                          %color.35))))
                      (let* (((%tmp.41
                                (tunsigned
                                  340282366920938463463374607431768211455)) (safe-cast
                                                                              (tunsigned
                                                                                340282366920938463463374607431768211455)
                                                                              (tunsigned
                                                                                18446744073709551615)
                                                                              (var-ref
                                                                                %amount.36))))
                        (public-ledger %kernel.24 update () incUnshieldedInputs (ttuple)
                          (instructions (swap (n 0))
                            (idx (cached #t)
                                 (pushPath #t)
                                 (path ((align 6 1))))
                            (push
                              (storage #f)
                              (value (state-value cell (var-ref %tmp.40))))
                            (dup (n 1)) (dup (n 1)) (member)
                            (push
                              (storage #f)
                              (value (state-value cell (var-ref %tmp.41))))
                            (swap (n 0)) (neg) (branch (skip 4))
                            (dup (n 2)) (dup (n 2))
                            (idx (cached #t)
                                 (pushPath #f)
                                 (path ((stack))))
                            (add) (ins (cached #t) (n 2)) (swap (n 0)))
                          (var-ref %tmp.40) (var-ref %tmp.41))))
                    (tuple))
                (return (var-ref %color.35))))))
  (circuit %unshieldedBalanceGt.14 (exported #f) (pure #f) (proof #f)
    ((%color.28 (tbytes 32))
      (%amount.30
        (tunsigned 340282366920938463463374607431768211455)))
    (tboolean)
    (return
      (let* (((%tmp.29
                (tstruct
                  Either
                  (is_left (tboolean))
                  (left (tbytes 32))
                  (right (tbytes 32)))) (call
                                          %left.27
                                          (var-ref %color.28))))
        (public-ledger %kernel.24 read () balanceGreaterThan (tboolean)
          (instructions
            (push
              (storage #f)
              (value (state-value cell (var-ref %amount.30))))
            (dup (n 3))
            (idx (cached #t) (pushPath #f) (path ((align 5 1))))
            (dup (n 0))
            (push
              (storage #f)
              (value (state-value cell (var-ref %tmp.29))))
            (member) (branch (skip 3)) (pop)
            (push (storage #f) (value (state-value cell (align 0 16))))
            (jmp (skip 1))
            (idx (cached #t) (pushPath #f) (path ((var-ref %tmp.29))))
            (lt) (popeq (cached #t) (result (void))))
          (var-ref %tmp.29) (var-ref %amount.30)))))
  (native %persistentCommit.31
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments (tvector 2 (tbytes 32)))
    ((%value.47 (tvector 2 (tbytes 32))) (%rand.48 (tbytes 32)))
    (tbytes 32))
  (circuit %unlock.10 (exported #t) (pure #f) (proof #t)
    ((%after.19 (tunsigned 18446744073709551615))
      (%before.21 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (assert
           (call %blockTimeGt.18 (var-ref %after.19))
           "too early")
         (assert
           (call %blockTimeLt.20 (var-ref %before.21))
           "too late")
         (let* (((%tmp.17 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 1)
                                               '1)))
           (public-ledger %unlocked.11 update (0) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 0 1))))
               (addi (immediate (value->int (var-ref %tmp.17))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.17)))
         (return (tuple))))
  (circuit %mint.13 (exported #t) (pure #f) (proof #t)
    ((%domainSep.25 (tbytes 32))
      (%amount.26 (tunsigned 18446744073709551615)))
    (tbytes 32)
    (return
      (call
        %mintUnshieldedToken.22
        (var-ref %domainSep.25)
        (var-ref %amount.26)
        (call
          %left.23
          (public-ledger %kernel.24 read () self
            (tstruct ContractAddress (bytes (tbytes 32)))
            (instructions
              (dup (n 2))
              (idx (cached #t) (pushPath #f) (path ((align 0 1))))
              (popeq (cached #t) (result (void)))))))))
  (circuit %holds.12 (exported #t) (pure #f) (proof #t)
    ((%color.15 (tbytes 32))
      (%amount.16
        (tunsigned 340282366920938463463374607431768211455)))
    (ttuple)
    (seq (assert
           (call
             %unshieldedBalanceGt.14
             (var-ref %color.15)
             (var-ref %amount.16))
           "balance too low")
         (return (tuple)))))
