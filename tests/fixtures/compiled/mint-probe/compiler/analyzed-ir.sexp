(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (mint . %mint.0) (recip_echo . %recip_echo.1))
  (contract-types)
  (kernel-declaration (%kernel.27 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %tokenType.19 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.20 (tbytes 32))
      (%contractAddress.21
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.8
        (tuple
          (single (var-ref %domain_sep.20))
          (single (elt-ref (var-ref %contractAddress.21) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintShieldedToken.6 (exported #f) (pure #f) (proof #f)
    ((%domain_sep.24 (tbytes 32))
      (%value.25 (tunsigned 18446744073709551615))
      (%nonce.22 (tbytes 32))
      (%recipient.23
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
    (let* (((%coin.26
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
                                                                                %nonce.22)
                                                                              (call
                                                                                %tokenType.19
                                                                                (var-ref
                                                                                  %domain_sep.24)
                                                                                (public-ledger
                                                                                  %kernel.27
                                                                                  read
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
                                                                                  %value.25)))))
      (seq (public-ledger %kernel.27 update () mintShielded (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 4 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %domain_sep.24))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %value.25))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %domain_sep.24) (var-ref %value.25))
           (call
             %createZswapOutput.11
             (var-ref %coin.26)
             (var-ref %recipient.23))
           (let* (((%cm.28 (tbytes 32)) (call
                                          %coinCommitment.14
                                          (var-ref %coin.26)
                                          (var-ref %recipient.23))))
             (seq (public-ledger %kernel.27 update () claimZswapCoinSpend (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 2 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %cm.28))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %cm.28))
                  (if (if (if (elt-ref (var-ref %recipient.23) is_left 0)
                              '#f
                              '#t)
                          (== (tbytes 32)
                              (elt-ref
                                (elt-ref (var-ref %recipient.23) right 2)
                                bytes
                                0)
                              (elt-ref
                                (public-ledger %kernel.27 read () self
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
                      (public-ledger %kernel.27 update () claimZswapCoinReceive (ttuple)
                        (instructions (swap (n 0))
                          (idx (cached #t)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %cm.28))))
                          (push (storage #f) (value (state-value null)))
                          (ins (cached #t) (n 2)) (swap (n 0)))
                        (var-ref %cm.28))
                      (tuple))
                  (return (var-ref %coin.26)))))))
  (circuit %coinCommitment.14 (exported #f) (pure #t) (proof #f)
    ((%coin.15
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.16
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.17
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
             (var-ref %coin.15)
             (elt-ref (var-ref %recipient.16) is_left 0)
             (if (elt-ref (var-ref %recipient.16) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.16) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.16) right 2)
                   bytes
                   0))))))
  (native %persistentHash.17
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments
      (tstruct CoinPreimage (domain_sep (tbytes 21))
        (info
          (tstruct
            ShieldedCoinInfo
            (nonce (tbytes 32))
            (color (tbytes 32))
            (value
              (tunsigned 340282366920938463463374607431768211455))))
        (dataType (tboolean)) (data (tbytes 32))))
    ((%value.18
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
  (native %persistentCommit.8
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments (tvector 2 (tbytes 32)))
    ((%value.9 (tvector 2 (tbytes 32))) (%rand.10 (tbytes 32)))
    (tbytes 32))
  (native %createZswapOutput.11
    (entry "__compactRuntime.createZswapOutput" witness)
    (type-arguments)
    ((%coin.12
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.13
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple))
  (circuit %mint.0 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.4 (tbytes 32))
      (%value.5 (tunsigned 18446744073709551615))
      (%nonce.2 (tbytes 32))
      (%recipient.3
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple)
    (seq (call %mintShieldedToken.6 (var-ref %domain_sep.4)
           (var-ref %value.5) (var-ref %nonce.2)
           (var-ref %recipient.3))
         (return (tuple))))
  (circuit %recip_echo.1 (exported #t) (pure #t) (proof #f)
    ((%r.7
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
    (return (var-ref %r.7))))
