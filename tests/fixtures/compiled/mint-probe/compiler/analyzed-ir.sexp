(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (mint . %mint.12) (recip_echo . %recip_echo.13))
  (contract-types)
  (kernel-declaration (%kernel.32 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %tokenType.26 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.27 (tbytes 32))
      (%contractAddress.28
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.20
        (tuple
          (single (var-ref %domain_sep.27))
          (single (elt-ref (var-ref %contractAddress.28) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintShieldedToken.14 (exported #f) (pure #f) (proof #f)
    ((%domain_sep.31 (tbytes 32))
      (%value.29 (tunsigned 18446744073709551615))
      (%nonce.30 (tbytes 32))
      (%recipient.34
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
    (let* (((%coin.33
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
                                                                                %nonce.30)
                                                                              (call
                                                                                %tokenType.26
                                                                                (var-ref
                                                                                  %domain_sep.31)
                                                                                (public-ledger
                                                                                  %kernel.32
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
                                                                                  %value.29)))))
      (seq (public-ledger %kernel.32 update () mintShielded (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 4 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %domain_sep.31))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %value.29))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %domain_sep.31) (var-ref %value.29))
           (call
             %createZswapOutput.21
             (var-ref %coin.33)
             (var-ref %recipient.34))
           (let* (((%cm.35 (tbytes 32)) (call
                                          %coinCommitment.22
                                          (var-ref %coin.33)
                                          (var-ref %recipient.34))))
             (seq (public-ledger %kernel.32 update () claimZswapCoinSpend (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 2 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %cm.35))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %cm.35))
                  (if (if (if (elt-ref (var-ref %recipient.34) is_left 0)
                              '#f
                              '#t)
                          (== (tbytes 32)
                              (elt-ref
                                (elt-ref (var-ref %recipient.34) right 2)
                                bytes
                                0)
                              (elt-ref
                                (public-ledger %kernel.32 read () self
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
                      (public-ledger %kernel.32 update () claimZswapCoinReceive (ttuple)
                        (instructions (swap (n 0))
                          (idx (cached #t)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %cm.35))))
                          (push (storage #f) (value (state-value null)))
                          (ins (cached #t) (n 2)) (swap (n 0)))
                        (var-ref %cm.35))
                      (tuple))
                  (return (var-ref %coin.33)))))))
  (circuit %coinCommitment.22 (exported #f) (pure #t) (proof #f)
    ((%coin.25
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.24
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.23
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
             (var-ref %coin.25)
             (elt-ref (var-ref %recipient.24) is_left 0)
             (if (elt-ref (var-ref %recipient.24) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.24) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.24) right 2)
                   bytes
                   0))))))
  (native %persistentHash.23
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
    ((%value.36
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
  (native %persistentCommit.20
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments (tvector 2 (tbytes 32)))
    ((%value.37 (tvector 2 (tbytes 32))) (%rand.38 (tbytes 32)))
    (tbytes 32))
  (native %createZswapOutput.21
    (entry "__compactRuntime.createZswapOutput" witness)
    (type-arguments)
    ((%coin.39
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.40
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple))
  (circuit %mint.12 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.17 (tbytes 32))
      (%value.18 (tunsigned 18446744073709551615))
      (%nonce.15 (tbytes 32))
      (%recipient.16
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (ttuple)
    (seq (call %mintShieldedToken.14 (var-ref %domain_sep.17)
           (var-ref %value.18) (var-ref %nonce.15)
           (var-ref %recipient.16))
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
