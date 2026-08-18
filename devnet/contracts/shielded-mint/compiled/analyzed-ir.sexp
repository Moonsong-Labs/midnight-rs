(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107") (exports (mint . %mint.0))
  (contract-types)
  (kernel-declaration (%kernel.14 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %left.4 (exported #f) (pure #t) (proof #f)
    ((%value.21
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
           (var-ref %value.21)
           (default (tstruct ContractAddress (bytes (tbytes 32)))))))
  (circuit %tokenType.12 (exported #f) (pure #t) (proof #f)
    ((%domain_sep.22 (tbytes 32))
      (%contractAddress.23
        (tstruct ContractAddress (bytes (tbytes 32)))))
    (tbytes 32)
    (return
      (call
        %persistentCommit.9
        (tuple
          (single (var-ref %domain_sep.22))
          (single (elt-ref (var-ref %contractAddress.23) bytes 0)))
        '#vu8(109 105 100 110 105 103 104 116 58 100 101 114 105 118
              101 95 116 111 107 101 110 0 0 0 0 0 0 0 0 0 0 0))))
  (circuit %mintShieldedToken.2 (exported #f) (pure #f) (proof #f)
    ((%domain_sep.13 (tbytes 32))
      (%value.10 (tunsigned 18446744073709551615))
      (%nonce.11 (tbytes 32))
      (%recipient.17
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
    (let* (((%coin.16
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
                                                                                %nonce.11)
                                                                              (call
                                                                                %tokenType.12
                                                                                (var-ref
                                                                                  %domain_sep.13)
                                                                                (public-ledger
                                                                                  %kernel.14
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
                                                                                  %value.10)))))
      (seq (public-ledger %kernel.14 update () mintShielded (ttuple)
             (instructions (swap (n 0))
               (idx (cached #t) (pushPath #t) (path ((align 4 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %domain_sep.13))))
               (dup (n 1)) (dup (n 1)) (member)
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %value.10))))
               (swap (n 0)) (neg) (branch (skip 4)) (dup (n 2)) (dup (n 2))
               (idx (cached #t) (pushPath #f) (path ((stack)))) (add)
               (ins (cached #t) (n 2)) (swap (n 0)))
             (var-ref %domain_sep.13) (var-ref %value.10))
           (call
             %createZswapOutput.1
             (var-ref %coin.16)
             (var-ref %recipient.17))
           (let* (((%cm.18 (tbytes 32)) (call
                                          %coinCommitment.15
                                          (var-ref %coin.16)
                                          (var-ref %recipient.17))))
             (seq (public-ledger %kernel.14 update () claimZswapCoinSpend (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 2 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %cm.18))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %cm.18))
                  (if (if (if (elt-ref (var-ref %recipient.17) is_left 0)
                              '#f
                              '#t)
                          (== (tbytes 32)
                              (elt-ref
                                (elt-ref (var-ref %recipient.17) right 2)
                                bytes
                                0)
                              (elt-ref
                                (public-ledger %kernel.14 read () self
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
                      (public-ledger %kernel.14 update () claimZswapCoinReceive (ttuple)
                        (instructions (swap (n 0))
                          (idx (cached #t)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %cm.18))))
                          (push (storage #f) (value (state-value null)))
                          (ins (cached #t) (n 2)) (swap (n 0)))
                        (var-ref %cm.18))
                      (tuple))
                  (return (var-ref %coin.16)))))))
  (circuit %coinCommitment.15 (exported #f) (pure #t) (proof #f)
    ((%coin.20
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.19
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.8
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
             (var-ref %coin.20)
             (elt-ref (var-ref %recipient.19) is_left 0)
             (if (elt-ref (var-ref %recipient.19) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.19) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.19) right 2)
                   bytes
                   0))))))
  (native %persistentHash.8
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
    ((%value.24
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
  (native %persistentCommit.9
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments (tvector 2 (tbytes 32)))
    ((%value.25 (tvector 2 (tbytes 32))) (%rand.26 (tbytes 32)))
    (tbytes 32))
  (native %createZswapOutput.1
    (entry "__compactRuntime.createZswapOutput" witness)
    (type-arguments)
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
    (ttuple))
  (circuit %mint.0 (exported #t) (pure #f) (proof #t)
    ((%domain_sep.6 (tbytes 32))
      (%value.7 (tunsigned 18446744073709551615))
      (%nonce.3 (tbytes 32))
      (%coinPK.5
        (tstruct ZswapCoinPublicKey (bytes (tbytes 32)))))
    (ttuple)
    (seq (call %mintShieldedToken.2 (var-ref %domain_sep.6)
           (var-ref %value.7) (var-ref %nonce.3)
           (call %left.4 (var-ref %coinPK.5)))
         (return (tuple)))))
