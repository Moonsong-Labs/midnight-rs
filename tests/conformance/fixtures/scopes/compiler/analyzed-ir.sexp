(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (cnt . %cnt.2)
    (entries . %entries.3)
    (other . %other.0)
    (readTwice . %readTwice.1))
  (contract-types)
  (kernel-declaration (%kernel.8 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%cnt.2 (0) (exported #t) (Counter))
      (%other.0 (1) (exported #t) (Counter))
      (%entries.3
        (2)
        (exported #t)
        (Map (tunsigned 18446744073709551615)
             (tunsigned 18446744073709551615))))
    (constructor () (tuple)))
  (circuit %readTwice.1 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (let* (((%tmp.6 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
           (public-ledger %cnt.2 update (0) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 0 1))))
               (addi (immediate (value->int (var-ref %tmp.6))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.6)))
         (let* (((%tmp.7 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 7)
                                              '7)))
           (public-ledger %other.0 update (1) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 1 1))))
               (addi (immediate (value->int (var-ref %tmp.7))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.7)))
         (let* (((%tmp.4 (tunsigned 18446744073709551615)) (public-ledger %cnt.2 read
                                                             (0) read
                                                             (tunsigned
                                                               18446744073709551615)
                                                             (instructions
                                                               (dup (n 0))
                                                               (idx (cached
                                                                      #f)
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
                                                                   (void)))))))
           (let* (((%tmp.5 (tunsigned 18446744073709551615)) (public-ledger %other.0
                                                               read (1)
                                                               read
                                                               (tunsigned
                                                                 18446744073709551615)
                                                               (instructions
                                                                 (dup (n 0))
                                                                 (idx (cached
                                                                        #f)
                                                                      (pushPath
                                                                        #f)
                                                                      (path
                                                                        ((align
                                                                           1
                                                                           1))))
                                                                 (popeq
                                                                   (cached
                                                                     #t)
                                                                   (result
                                                                     (void)))))))
             (public-ledger %entries.3 update (2) insert (ttuple)
               (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                 (push
                   (storage #f)
                   (value (state-value cell (var-ref %tmp.4))))
                 (push
                   (storage #t)
                   (value
                     (state-value
                       ADT
                       (var-ref %tmp.5)
                       (tunsigned 18446744073709551615))))
                 (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
               (var-ref %tmp.4) (var-ref %tmp.5))))
         (return (tuple)))))
