(normalized-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107") (exports (mint . %mint.0))
  (contract-types)
  (kernel-declaration (%kernel.21 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %left.9 (exported #f) (pure #t) (proof #f)
    ((%value.26
       (tstruct ZswapCoinPublicKey (bytes (tbytes 32)))))
    (tstruct
      Either
      (is_left (tboolean))
      (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
      (right (tstruct ContractAddress (bytes (tbytes 32)))))
    (return
      (new (tstruct
             Either
             (is_left (tboolean))
             (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
             (right (tstruct ContractAddress (bytes (tbytes 32)))))
           '#t
           (var-ref %value.26)
           (default (tstruct ContractAddress (bytes (tbytes 32)))))))
  (circuit %tokenType.20 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.27 (tbytes 32))
      (%contractAddress.28
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.12
        (tuple
          (single (var-ref %domain_sep.27))
          (single (elt-ref (var-ref %contractAddress.28) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintShieldedToken.8 (exported #f) (pure #f) (proof #f)
    ((%domain_sep.17 (tbytes 32))
      (%value.18 (tunsigned 18446744073709551615))
      (%nonce.15 (tbytes 32))
      (%recipient.16
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tstruct
      ShieldedCoinInfo
      (nonce (tbytes 32))
      (color (tbytes 32))
      (value (tunsigned 340282366920938463463374607431768211455)))
    (let* (((%coin.19
              (tstruct
                ShieldedCoinInfo
                (nonce (tbytes 32))
                (color (tbytes 32))
                (value
                  (tunsigned 340282366920938463463374607431768211455)))) (new (tstruct
                                                                                ShieldedCoinInfo
                                                                                (nonce
                                                                                  (tbytes
                                                                                    32))
                                                                                (color
                                                                                  (tbytes
                                                                                    32))
                                                                                (value
                                                                                  (tunsigned
                                                                                    340282366920938463463374607431768211455)))
                                                                              (var-ref
                                                                                %nonce.15)
                                                                              (call
                                                                                %tokenType.20
                                                                                (var-ref
                                                                                  %domain_sep.17)
                                                                                (public-ledger
                                                                                  %kernel.21
                                                                                  ()
                                                                                  self
                                                                                  (tstruct
                                                                                    ContractAddress
                                                                                    (bytes
                                                                                      (tbytes
                                                                                        32)))
                                                                                  (instructions
                                                                                    (dup (n 2))
                                                                                    (idx (cached
                                                                                           #t)
                                                                                         (pushPath
                                                                                           #f)
                                                                                         (path
                                                                                           ((align
                                                                                              0
                                                                                              1))))
                                                                                    (popeq
                                                                                      (cached
                                                                                        #t)
                                                                                      (result
                                                                                        (void))))))
                                                                              (safe-cast
                                                                                (tunsigned
                                                                                  340282366920938463463374607431768211455)
                                                                                (tunsigned
                                                                                  18446744073709551615)
                                                                                (var-ref
                                                                                  %value.18)))))
      (seq (public-ledger %kernel.21 () mintShielded (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 4 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %domain_sep.17))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %value.18))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %domain_sep.17) (var-ref %value.18))
           (call
             %createZswapOutput.1
             (var-ref %coin.19)
             (var-ref %recipient.16))
           (let* (((%cm.22 (tbytes 32)) (call
                                          %coinCommitment.23
                                          (var-ref %coin.19)
                                          (var-ref %recipient.16))))
             (seq (public-ledger %kernel.21 () claimZswapCoinSpend (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 2 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %cm.22))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %cm.22))
                  (if (if (if (elt-ref (var-ref %recipient.16) is_left 0)
                              '#f
                              '#t)
                          (== (tbytes 32)
                              (elt-ref
                                (elt-ref (var-ref %recipient.16) right 2)
                                bytes
                                0)
                              (elt-ref
                                (public-ledger %kernel.21 () self
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
                      (public-ledger %kernel.21 () claimZswapCoinReceive (ttuple)
                        (instructions (swap (n 0))
                          (idx (cached #t)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %cm.22))))
                          (push (storage #f) (value (state-value null)))
                          (ins (cached #t) (n 2)) (swap (n 0)))
                        (var-ref %cm.22))
                      (tuple))
                  (return (var-ref %coin.19)))))))
  (circuit %coinCommitment.23 (exported #f) (pure #t) (proof #f)
    ((%coin.24
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.25
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.10
        (new (tstruct CoinPreimage (domain_sep (tbytes 21))
               (info
                 (tstruct
                   ShieldedCoinInfo
                   (nonce (tbytes 32))
                   (color (tbytes 32))
                   (value
                     (tunsigned 340282366920938463463374607431768211455))))
               (dataType (tboolean)) (data (tbytes 32)))
             '#vu8(109 105 100 110 105 103 104 116 58 122 115 119 97 112
                   45 99 99 91 118 49 93)
             (var-ref %coin.24)
             (elt-ref (var-ref %recipient.25) is_left 0)
             (if (elt-ref (var-ref %recipient.25) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.25) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.25) right 2)
                   bytes
                   0))))))
  (native
    %persistentHash.10
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.11
       (tstruct CoinPreimage (domain_sep (tbytes 21))
         (info
           (tstruct
             ShieldedCoinInfo
             (nonce (tbytes 32))
             (color (tbytes 32))
             (value
               (tunsigned 340282366920938463463374607431768211455))))
         (dataType (tboolean)) (data (tbytes 32)))))
    (tbytes 32))
  (native
    %persistentCommit.12
    (entry "__compactRuntime.persistentCommit" circuit)
    ((%value.13 (tvector 2 (tbytes 32))) (%rand.14 (tbytes 32)))
    (tbytes 32))
  (native
    %createZswapOutput.1
    (entry "__compactRuntime.createZswapOutput" witness)
    ((%coin.2
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.3
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple))
  (circuit %mint.0 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.6 (tbytes 32))
      (%value.7 (tunsigned 18446744073709551615))
      (%nonce.4 (tbytes 32))
      (%coinPK.5
        (tstruct ZswapCoinPublicKey (bytes (tbytes 32)))))
    (ttuple)
    (seq (call %mintShieldedToken.8 (var-ref %domain_sep.6)
           (var-ref %value.7) (var-ref %nonce.4)
           (call %left.9 (var-ref %coinPK.5)))
         (return (tuple)))))
