(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (mint . %mint.12) (recip_echo . %recip_echo.13))
  (contract-types)
  (kernel-declaration (%kernel.39 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %tokenType.31 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.32 (tbytes 32))
      (%contractAddress.33
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.20
        (tuple
          (single (var-ref %domain_sep.32))
          (single (elt-ref (var-ref %contractAddress.33) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintShieldedToken.18 (exported #f) (pure #f) (proof #f)
    ((%domain_sep.36 (tbytes 32))
      (%value.37 (tunsigned 18446744073709551615))
      (%nonce.34 (tbytes 32))
      (%recipient.35
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
    (let* (((%coin.38
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
                                                                                %nonce.34)
                                                                              (call
                                                                                %tokenType.31
                                                                                (var-ref
                                                                                  %domain_sep.36)
                                                                                (public-ledger
                                                                                  %kernel.39
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
                                                                                  %value.37)))))
      (seq (public-ledger %kernel.39 () mintShielded (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 4 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %domain_sep.36))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %value.37))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %domain_sep.36) (var-ref %value.37))
           (call
             %createZswapOutput.23
             (var-ref %coin.38)
             (var-ref %recipient.35))
           (let* (((%cm.40 (tbytes 32)) (call
                                          %coinCommitment.26
                                          (var-ref %coin.38)
                                          (var-ref %recipient.35))))
             (seq (public-ledger %kernel.39 () claimZswapCoinSpend (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 2 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %cm.40))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %cm.40))
                  (if (if (if (elt-ref (var-ref %recipient.35) is_left 0)
                              '#f
                              '#t)
                          (== (tbytes 32)
                              (elt-ref
                                (elt-ref (var-ref %recipient.35) right 2)
                                bytes
                                0)
                              (elt-ref
                                (public-ledger %kernel.39 () self
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
                      (public-ledger %kernel.39 () claimZswapCoinReceive (ttuple)
                        (instructions (swap (n 0))
                          (idx (cached #t)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %cm.40))))
                          (push (storage #f) (value (state-value null)))
                          (ins (cached #t) (n 2)) (swap (n 0)))
                        (var-ref %cm.40))
                      (tuple))
                  (return (var-ref %coin.38)))))))
  (circuit %coinCommitment.26 (exported #f) (pure #t) (proof #f)
    ((%coin.27
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.28
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.29
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
             (var-ref %coin.27)
             (elt-ref (var-ref %recipient.28) is_left 0)
             (if (elt-ref (var-ref %recipient.28) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.28) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.28) right 2)
                   bytes
                   0))))))
  (native
    %persistentHash.29
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.30
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
    %persistentCommit.20
    (entry "__compactRuntime.persistentCommit" circuit)
    ((%value.21 (tvector 2 (tbytes 32))) (%rand.22 (tbytes 32)))
    (tbytes 32))
  (native
    %createZswapOutput.23
    (entry "__compactRuntime.createZswapOutput" witness)
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
    (ttuple))
  (circuit %mint.12 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.16 (tbytes 32))
      (%value.17 (tunsigned 18446744073709551615))
      (%nonce.14 (tbytes 32))
      (%recipient.15
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple)
    (seq (call %mintShieldedToken.18 (var-ref %domain_sep.16)
           (var-ref %value.17) (var-ref %nonce.14)
           (var-ref %recipient.15))
         (return (tuple))))
  (circuit %recip_echo.13 (exported #t) (pure #t) (proof #f)
    ((%r.19
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tstruct
      Either
      (is_left (tboolean))
      (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
      (right (tstruct ContractAddress (bytes (tbytes 32)))))
    (return (var-ref %r.19))))
