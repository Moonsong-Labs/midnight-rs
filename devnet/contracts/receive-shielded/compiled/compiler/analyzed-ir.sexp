(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (deposit . %deposit.0)) (contract-types)
  (kernel-declaration (%kernel.10 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array)
    (constructor () (tuple)))
  (circuit %right.8 (exported #f) (pure #t) (proof #f)
    ((%value.9 (tstruct ContractAddress (bytes (tbytes 32)))))
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
           '#f
           (default (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
           (var-ref %value.9))))
  (circuit %receiveShielded.2 (exported #f) (pure #f) (proof #f)
    ((%coin.11
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455)))))
    (ttuple)
    (seq (let* (((%recipient.12
                   (tstruct
                     Either
                     (is_left (tboolean))
                     (left
                       (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
                     (right
                       (tstruct ContractAddress (bytes (tbytes 32)))))) (call
                                                                          %right.8
                                                                          (public-ledger
                                                                            %kernel.10
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
                                                                                  (void))))))))
           (seq (call
                  %createZswapOutput.1
                  (var-ref %coin.11)
                  (var-ref %recipient.12))
                (let* (((%tmp.13 (tbytes 32)) (call
                                                %coinCommitment.4
                                                (var-ref %coin.11)
                                                (var-ref %recipient.12))))
                  (public-ledger %kernel.10 update () claimZswapCoinReceive (ttuple)
                    (instructions (swap (n 0))
                      (idx (cached #t) (pushPath #t) (path ((align 1 1))))
                      (push
                        (storage #f)
                        (value (state-value cell (var-ref %tmp.13))))
                      (push (storage #f) (value (state-value null)))
                      (ins (cached #t) (n 2)) (swap (n 0)))
                    (var-ref %tmp.13)))))
         (return (tuple))))
  (circuit %coinCommitment.4 (exported #f) (pure #t) (proof #f)
    ((%coin.7
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455))))
      (%recipient.6
        (tstruct
          Either
          (is_left (tboolean))
          (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (right (tstruct ContractAddress (bytes (tbytes 32)))))))
    (tbytes 32)
    (return
      (call
        %persistentHash.5
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
             (var-ref %coin.7) (elt-ref (var-ref %recipient.6) is_left 0)
             (if (elt-ref (var-ref %recipient.6) is_left 0)
                 (elt-ref (elt-ref (var-ref %recipient.6) left 1) bytes 0)
                 (elt-ref
                   (elt-ref (var-ref %recipient.6) right 2)
                   bytes
                   0))))))
  (native %persistentHash.5
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
    ((%value.14
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
  (native %createZswapOutput.1
    (entry "__compactRuntime.createZswapOutput" witness)
    (type-arguments)
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
    (ttuple))
  (circuit %deposit.0 (exported #t) (pure #f) (proof #t)
    ((%coin.3
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455)))))
    (ttuple)
    (seq (call %receiveShielded.2 (var-ref %coin.3))
         (return (tuple)))))
