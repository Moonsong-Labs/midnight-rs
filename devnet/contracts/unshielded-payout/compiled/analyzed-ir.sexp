(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (mintToSelf . %mintToSelf.7)
    (payUser . %payUser.8))
  (contract-types)
  (kernel-declaration (%kernel.17 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %left.16 (exported #f) (pure #t) (proof #f)
    ((%value.39 (tstruct ContractAddress (bytes (tbytes 32)))))
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
           (var-ref %value.39)
           (default (tstruct UserAddress (bytes (tbytes 32)))))))
  (circuit %left.23 (exported #f) (pure #t) (proof #f)
    ((%value.40 (tbytes 32)))
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
           (var-ref %value.40)
           (default (tbytes 32)))))
  (circuit %right.10 (exported #f) (pure #t) (proof #f)
    ((%value.36 (tstruct UserAddress (bytes (tbytes 32)))))
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
           '#f
           (default (tstruct ContractAddress (bytes (tbytes 32))))
           (var-ref %value.36))))
  (circuit %tokenType.20 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.37 (tbytes 32))
      (%contractAddress.38
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.14
        (tuple
          (single (var-ref %domain_sep.37))
          (single (elt-ref (var-ref %contractAddress.38) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintUnshieldedToken.15 (exported #f) (pure #f) (proof #f)
    ((%domainSep.21 (tbytes 32))
      (%amount.24 (tunsigned 18446744073709551615))
      (%recipient.27
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ContractAddress (bytes (tbytes 32))))
          (right (tstruct UserAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (seq (public-ledger %kernel.17 update () mintUnshielded (ttuple)
           (instructions (swap (n 0))
             (idx (cached #t) (pushPath #t) (path ((align 5 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %domainSep.21))))
             (dup (n 1)) (dup (n 1)) (member)
             (push
               (storage #f)
               (value (state-value cell (var-ref %amount.24))))
             (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
             (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
             (ins (cached #t) (n 2)) (swap (n 0)))
           (var-ref %domainSep.21) (var-ref %amount.24))
         (let* (((%color.22 (tbytes 32)) (call
                                           %tokenType.20
                                           (var-ref %domainSep.21)
                                           (public-ledger %kernel.17 read () self
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
           (seq (let* (((%tmp.26
                          (tstruct
                            Either
                            (is_left (tboolean))
                            (left (tbytes 32))
                            (right (tbytes 32)))) (call
                                                    %left.23
                                                    (var-ref %color.22))))
                  (let* (((%tmp.25
                            (tunsigned
                              340282366920938463463374607431768211455)) (safe-cast
                                                                          (tunsigned
                                                                            340282366920938463463374607431768211455)
                                                                          (tunsigned
                                                                            18446744073709551615)
                                                                          (var-ref
                                                                            %amount.24))))
                    (public-ledger %kernel.17 update () claimUnshieldedCoinSpend
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
                                (var-ref %tmp.26)
                                (var-ref %recipient.27)))))
                        (dup (n 1)) (dup (n 1)) (member)
                        (push
                          (storage #f)
                          (value (state-value cell (var-ref %tmp.25))))
                        (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2))
                        (dup (n 2))
                        (idx (cached #t) (pushPath #f) (path ((stack))))
                        (add) (ins (cached #t) (n 2)) (swap (n 0)))
                      (var-ref %tmp.26) (var-ref %recipient.27)
                      (var-ref %tmp.25))))
                (if (if (elt-ref (var-ref %recipient.27) is_left 0)
                        (== (tbytes 32)
                            (elt-ref
                              (elt-ref (var-ref %recipient.27) left 1)
                              bytes
                              0)
                            (elt-ref
                              (public-ledger %kernel.17 read () self
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
                    (let* (((%tmp.28
                              (tstruct
                                Either
                                (is_left (tboolean))
                                (left (tbytes 32))
                                (right (tbytes 32)))) (call
                                                        %left.23
                                                        (var-ref
                                                          %color.22))))
                      (let* (((%tmp.29
                                (tunsigned
                                  340282366920938463463374607431768211455)) (safe-cast
                                                                              (tunsigned
                                                                                340282366920938463463374607431768211455)
                                                                              (tunsigned
                                                                                18446744073709551615)
                                                                              (var-ref
                                                                                %amount.24))))
                        (public-ledger %kernel.17 update () incUnshieldedInputs (ttuple)
                          (instructions (swap (n 0))
                            (idx (cached #t)
                                 (pushPath #t)
                                 (path ((align 6 1))))
                            (push
                              (storage #f)
                              (value (state-value cell (var-ref %tmp.28))))
                            (dup (n 1)) (dup (n 1)) (member)
                            (push
                              (storage #f)
                              (value (state-value cell (var-ref %tmp.29))))
                            (swap (n 0)) (neg) (branch (skip 4))
                            (dup (n 2)) (dup (n 2))
                            (idx (cached #t)
                                 (pushPath #f)
                                 (path ((stack))))
                            (add) (ins (cached #t) (n 2)) (swap (n 0)))
                          (var-ref %tmp.28) (var-ref %tmp.29))))
                    (tuple))
                (return (var-ref %color.22))))))
  (circuit %sendUnshielded.9 (exported #f) (pure #f) (proof #f)
    ((%color.31 (tbytes 32))
      (%amount.33
        (tunsigned 340282366920938463463374607431768211455))
      (%recipient.30
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ContractAddress (bytes (tbytes 32))))
          (right (tstruct UserAddress (bytes (tbytes 32)))))))
    (ttuple)
    (seq (let* (((%tmp.34
                   (tstruct
                     Either
                     (is_left (tboolean))
                     (left (tbytes 32))
                     (right (tbytes 32)))) (call
                                             %left.23
                                             (var-ref %color.31))))
           (public-ledger %kernel.17 update () incUnshieldedOutputs (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 7 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %tmp.34))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %amount.33))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %tmp.34) (var-ref %amount.33)))
         (let* (((%tmp.35
                   (tstruct
                     Either
                     (is_left (tboolean))
                     (left (tbytes 32))
                     (right (tbytes 32)))) (call
                                             %left.23
                                             (var-ref %color.31))))
           (public-ledger %kernel.17 update () claimUnshieldedCoinSpend (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 8 1))))
               (push
                 (storage #f)
                 (value
                   (state-value
                     cell
                     (aligned-concat
                       (var-ref %tmp.35)
                       (var-ref %recipient.30)))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %amount.33))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %tmp.35) (var-ref %recipient.30)
             (var-ref %amount.33)))
         (if (if (elt-ref (var-ref %recipient.30) is_left 0)
                 (== (tbytes 32)
                     (elt-ref
                       (elt-ref (var-ref %recipient.30) left 1)
                       bytes
                       0)
                     (elt-ref
                       (public-ledger %kernel.17 read () self
                         (tstruct ContractAddress (bytes (tbytes 32)))
                         (instructions
                           (dup (n 2))
                           (idx (cached #t)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #t) (result (void)))))
                       bytes
                       0))
                 '#f)
             (let* (((%tmp.32
                       (tstruct
                         Either
                         (is_left (tboolean))
                         (left (tbytes 32))
                         (right (tbytes 32)))) (call
                                                 %left.23
                                                 (var-ref %color.31))))
               (public-ledger %kernel.17 update () incUnshieldedInputs (ttuple)
                 (instructions (swap (n 0))
                   (idx (cached #t) (pushPath #t) (path ((align 6 1))))
                   (push
                     (storage #f)
                     (value (state-value cell (var-ref %tmp.32))))
                   (dup (n 1)) (dup (n 1)) (member)
                   (push
                     (storage #f)
                     (value (state-value cell (var-ref %amount.33))))
                   (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2))
                   (dup (n 2))
                   (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
                   (ins (cached #t) (n 2)) (swap (n 0)))
                 (var-ref %tmp.32) (var-ref %amount.33)))
             (tuple))
         (return (tuple))))
  (native %persistentCommit.14
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments (tvector 2 (tbytes 32)))
    ((%value.41 (tvector 2 (tbytes 32))) (%rand.42 (tbytes 32)))
    (tbytes 32))
  (circuit %mintToSelf.7 (exported #t) (pure #f) (proof #t)
    ((%domainSep.18 (tbytes 32))
      (%amount.19 (tunsigned 18446744073709551615)))
    (tbytes 32)
    (return
      (call
        %mintUnshieldedToken.15
        (var-ref %domainSep.18)
        (var-ref %amount.19)
        (call
          %left.16
          (public-ledger %kernel.17 read () self
            (tstruct ContractAddress (bytes (tbytes 32)))
            (instructions
              (dup (n 2))
              (idx (cached #t) (pushPath #f) (path ((align 0 1))))
              (popeq (cached #t) (result (void)))))))))
  (circuit %payUser.8 (exported #t) (pure #f) (proof #t)
    ((%color.12 (tbytes 32))
      (%amount.13
        (tunsigned 340282366920938463463374607431768211455))
      (%address.11 (tstruct UserAddress (bytes (tbytes 32)))))
    (ttuple)
    (seq (call
           %sendUnshielded.9
           (var-ref %color.12)
           (var-ref %amount.13)
           (call %right.10 (var-ref %address.11)))
         (return (tuple)))))
