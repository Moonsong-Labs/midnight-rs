(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (add_entries . %add_entries.12) (clear_all . %clear_all.13)
    (cycle_queue . %cycle_queue.10)
    (drop_entries . %drop_entries.11) (measure . %measure.8)
    (queue . %queue.9) (rounds . %rounds.6) (scores . %scores.7)
    (tags . %tags.4) (wind_back . %wind_back.5))
  (contract-types)
  (kernel-declaration (%kernel.24 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%tags.4 (0) (exported #t) (Set (tbytes 32)))
      (%scores.7
        (1)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 18446744073709551615)))
      (%queue.9
        (2)
        (exported #t)
        (List (tunsigned 18446744073709551615)))
      (%rounds.6 (3) (exported #t) (Counter)))
    (constructor () (tuple)))
  (circuit %add_entries.12 (exported #t) (pure #f) (proof #t)
    ((%tag.19 (tbytes 32))
      (%key.20 (tunsigned 255))
      (%score.21 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (public-ledger %tags.4 update (0) insert (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %tag.19))))
             (push (storage #t) (value (state-value null)))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %tag.19))
         (public-ledger %scores.7 update (1) insert (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %key.20))))
             (push
               (storage #t)
               (value
                 (state-value
                   ADT
                   (var-ref %score.21)
                   (tunsigned 18446744073709551615))))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %key.20) (var-ref %score.21))
         (return (tuple))))
  (circuit %drop_entries.11 (exported #t) (pure #f) (proof #t)
    ((%tag.22 (tbytes 32)) (%key.23 (tunsigned 255))) (ttuple)
    (seq (public-ledger %tags.4 remove (0) remove (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %tag.22))))
             (rem (cached #f))
             (ins (cached #t) (n 1)))
           (var-ref %tag.22))
         (public-ledger %scores.7 remove (1) remove (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %key.23))))
             (rem (cached #f))
             (ins (cached #t) (n 1)))
           (var-ref %key.23))
         (return (tuple))))
  (circuit %measure.8 (exported #t) (pure #f) (proof #t) ()
    (tunsigned 18446744073709551615)
    (seq (assert
           (== (tboolean)
               (public-ledger %tags.4 read (0) isEmpty (tboolean)
                 (instructions (dup (n 0))
                   (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                   (size)
                   (push
                     (storage #f)
                     (value (state-value cell (align 0 8))))
                   (eq) (popeq (cached #t) (result (void)))))
               '#f)
           "measure: tags is empty")
         (assert
           (== (tboolean)
               (public-ledger %scores.7 read (1) isEmpty (tboolean)
                 (instructions (dup (n 0))
                   (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                   (size)
                   (push
                     (storage #f)
                     (value (state-value cell (align 0 8))))
                   (eq) (popeq (cached #t) (result (void)))))
               '#f)
           "measure: scores is empty")
         (return
           (public-ledger %tags.4 read (0) size (tunsigned 18446744073709551615)
             (instructions
               (dup (n 0))
               (idx (cached #f) (pushPath #f) (path ((align 0 1))))
               (size)
               (popeq (cached #t) (result (void))))))))
  (circuit %cycle_queue.10 (exported #t) (pure #f) (proof #t)
    ((%v.18 (tunsigned 18446744073709551615)))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tunsigned 18446744073709551615)))
    (seq (public-ledger %queue.9 update (2) pushFront (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
             (dup (n 0))
             (idx (cached #f) (pushPath #f) (path ((align 2 1))))
             (addi (immediate 1))
             (push
               (storage #t)
               (value
                 (state-value
                   array
                   (state-value cell (var-ref %v.18))
                   (state-value null)
                   (state-value null))))
             (swap (n 0))
             (push (storage #f) (value (state-value cell (align 2 1))))
             (swap (n 0)) (ins (cached #t) (n 1)) (swap (n 0))
             (push (storage #f) (value (state-value cell (align 1 1))))
             (swap (n 0)) (ins (cached #t) (n 2)))
           (var-ref %v.18))
         (let* (((%front.17
                   (tstruct
                     Maybe
                     (is_some (tboolean))
                     (value (tunsigned 18446744073709551615)))) (public-ledger %queue.9
                                                                  read (2)
                                                                  head
                                                                  (tstruct
                                                                    Maybe
                                                                    (is_some
                                                                      (tboolean))
                                                                    (value
                                                                      (tunsigned
                                                                        18446744073709551615)))
                                                                  (instructions
                                                                    (dup (n 0))
                                                                    (idx (cached
                                                                           #f)
                                                                         (pushPath
                                                                           #f)
                                                                         (path
                                                                           ((align
                                                                              2
                                                                              1))))
                                                                    (idx (cached
                                                                           #f)
                                                                         (pushPath
                                                                           #f)
                                                                         (path
                                                                           ((align
                                                                              0
                                                                              1))))
                                                                    (dup (n 0))
                                                                    (type)
                                                                    (push
                                                                      (storage
                                                                        #f)
                                                                      (value
                                                                        (state-value
                                                                          cell
                                                                          (align
                                                                            1
                                                                            1))))
                                                                    (eq)
                                                                    (branch
                                                                      (skip
                                                                        4))
                                                                    (push
                                                                      (storage
                                                                        #f)
                                                                      (value
                                                                        (state-value
                                                                          cell
                                                                          (align
                                                                            1
                                                                            1))))
                                                                    (swap
                                                                      (n 0))
                                                                    (concat
                                                                      (cached
                                                                        #f)
                                                                      (n (+ 2
                                                                            (max-sizeof
                                                                              (tunsigned
                                                                                18446744073709551615)))))
                                                                    (jmp (skip
                                                                           2))
                                                                    (pop)
                                                                    (push
                                                                      (storage
                                                                        #f)
                                                                      (value
                                                                        (state-value
                                                                          cell
                                                                          (aligned-concat
                                                                            (align
                                                                              0
                                                                              1)
                                                                            (null
                                                                              (tunsigned
                                                                                18446744073709551615))))))
                                                                    (popeq
                                                                      (cached
                                                                        #t)
                                                                      (result
                                                                        (void)))))))
           (seq (public-ledger %queue.9 remove (2) popFront (ttuple)
                  (instructions
                    (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                    (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                    (ins (cached #t) (n 1))))
                (return (var-ref %front.17))))))
  (circuit %wind_back.5 (exported #t) (pure #f) (proof #t)
    ((%n.16 (tunsigned 65535))
      (%threshold.14 (tunsigned 18446744073709551615)))
    (tboolean)
    (seq (let* (((%tmp.15 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 4)
                                               '4)))
           (public-ledger %rounds.6 update (3) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 3 1))))
               (addi (immediate (value->int (var-ref %tmp.15))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.15)))
         (public-ledger %rounds.6 update (3) decrement (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 3 1))))
             (subi (immediate (value->int (var-ref %n.16))))
             (ins (cached #t) (n 1)))
           (var-ref %n.16))
         (return
           (public-ledger %rounds.6 read (3) lessThan (tboolean)
             (instructions (dup (n 0))
               (idx (cached #f) (pushPath #f) (path ((align 3 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %threshold.14))))
               (lt) (popeq (cached #t) (result (void))))
             (var-ref %threshold.14)))))
  (circuit %clear_all.13 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (public-ledger %tags.4 remove (0) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push (storage #t) (value (state-value map)))
             (ins (cached #f) (n 1))))
         (public-ledger %scores.7 remove (1) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 1 1))))
             (push (storage #t) (value (state-value map)))
             (ins (cached #f) (n 1))))
         (public-ledger %queue.9 remove (2) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 2 1))))
             (push
               (storage #t)
               (value
                 (state-value
                   array
                   (state-value null)
                   (state-value null)
                   (state-value cell (align 0 8)))))
             (ins (cached #f) (n 1))))
         (return (tuple)))))
