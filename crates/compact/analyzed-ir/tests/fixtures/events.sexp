(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (count . %count.86) (mint_event . %mint_event.87))
  (contract-types)
  (kernel-declaration (%kernel.93 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%count.86 (0) (exported #t) (Counter)))
    (constructor () (tuple)))
  (circuit %mint_event.87 (exported #t) (pure #f) (proof #t)
    ((%domain.89 (tbytes 32))
      (%token.90 (tbytes 32))
      (%amount.88
        (tunsigned 340282366920938463463374607431768211455)))
    (ttuple)
    (seq (let* (((%tmp.91 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 1)
                                               '1)))
           (public-ledger %count.86 (0) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 0 1))))
               (addi (immediate (value->int (var-ref %tmp.91))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.91)))
         (emit 1 6 80
           (let* (((%t.92
                     (tstruct
                       UnshieldedMint
                       (domainSep (tbytes 32))
                       (tokenType (tbytes 32))
                       (amount
                         (tunsigned
                           340282366920938463463374607431768211455)))) (new (tstruct
                                                                              UnshieldedMint
                                                                              (domainSep
                                                                                (tbytes
                                                                                  32))
                                                                              (tokenType
                                                                                (tbytes
                                                                                  32))
                                                                              (amount
                                                                                (tunsigned
                                                                                  340282366920938463463374607431768211455)))
                                                                            (var-ref
                                                                              %domain.89)
                                                                            (var-ref
                                                                              %token.90)
                                                                            (var-ref
                                                                              %amount.88))))
             (vector->bytes
               80
               (vector
                 (spread
                   32
                   (bytes->vector
                     32
                     (elt-ref (var-ref %t.92) domainSep 0)))
                 (spread
                   32
                   (bytes->vector
                     32
                     (elt-ref (var-ref %t.92) tokenType 1)))
                 (spread
                   16
                   (bytes->vector
                     16
                     (field->bytes
                       16
                       (field-native)
                       (safe-cast
                         (tfield (field-native))
                         (tunsigned
                           340282366920938463463374607431768211455)
                         (elt-ref (var-ref %t.92) amount 2))))))))
           (instructions
             (push
               (storage #f)
               (value
                 (state-value
                   array
                   (state-value cell (align 1 4))
                   (state-value cell (align 6 1))
                   (state-value
                     cell
                     (let* (((%t.92
                               (tstruct
                                 UnshieldedMint
                                 (domainSep (tbytes 32))
                                 (tokenType (tbytes 32))
                                 (amount
                                   (tunsigned
                                     340282366920938463463374607431768211455)))) (new (tstruct
                                                                                        UnshieldedMint
                                                                                        (domainSep
                                                                                          (tbytes
                                                                                            32))
                                                                                        (tokenType
                                                                                          (tbytes
                                                                                            32))
                                                                                        (amount
                                                                                          (tunsigned
                                                                                            340282366920938463463374607431768211455)))
                                                                                      (var-ref
                                                                                        %domain.89)
                                                                                      (var-ref
                                                                                        %token.90)
                                                                                      (var-ref
                                                                                        %amount.88))))
                       (vector->bytes
                         80
                         (vector
                           (spread
                             32
                             (bytes->vector
                               32
                               (elt-ref (var-ref %t.92) domainSep 0)))
                           (spread
                             32
                             (bytes->vector
                               32
                               (elt-ref (var-ref %t.92) tokenType 1)))
                           (spread
                             16
                             (bytes->vector
                               16
                               (field->bytes
                                 16
                                 (field-native)
                                 (safe-cast
                                   (tfield (field-native))
                                   (tunsigned
                                     340282366920938463463374607431768211455)
                                   (elt-ref
                                     (var-ref %t.92)
                                     amount
                                     2))))))))))))
             (log)))
         (return (tuple)))))
