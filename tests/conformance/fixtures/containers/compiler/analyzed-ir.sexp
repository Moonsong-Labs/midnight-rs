(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (add_entries . %add_entries.15) (clear_all . %clear_all.16)
    (cycle_queue . %cycle_queue.13)
    (drop_entries . %drop_entries.14)
    (head_point . %head_point.11) (measure . %measure.12)
    (points . %points.9) (push_queue . %push_queue.10)
    (queue . %queue.7) (rounds . %rounds.8) (scores . %scores.5)
    (tags . %tags.6) (wind_back . %wind_back.4))
  (contract-types)
  (kernel-declaration (%kernel.28 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array (%tags.6 (0) (exported #t) (Set (tbytes 32)))
      (%scores.5
        (1)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 18446744073709551615)))
      (%queue.7
        (2)
        (exported #t)
        (List (tunsigned 18446744073709551615)))
      (%rounds.8 (3) (exported #t) (Counter))
      (%points.9
        (4)
        (exported #t)
        (List (tpoint (curve-jubjub)))))
    (constructor () (tuple)))
  (circuit %add_entries.15 (exported #t) (pure #f) (proof #t)
    ((%tag.23 (tbytes 32))
      (%key.24 (tunsigned 255))
      (%score.25 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (public-ledger %tags.6 update (0) insert (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %tag.23))))
             (push (storage #t) (value (state-value null)))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %tag.23))
         (public-ledger %scores.5 update (1) insert (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %key.24))))
             (push
               (storage #t)
               (value
                 (state-value
                   ADT
                   (var-ref %score.25)
                   (tunsigned 18446744073709551615))))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %key.24) (var-ref %score.25))
         (return (tuple))))
  (circuit %drop_entries.14 (exported #t) (pure #f) (proof #t)
    ((%tag.26 (tbytes 32)) (%key.27 (tunsigned 255))) (ttuple)
    (seq (public-ledger %tags.6 remove (0) remove (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %tag.26))))
             (rem (cached #f))
             (ins (cached #t) (n 1)))
           (var-ref %tag.26))
         (public-ledger %scores.5 remove (1) remove (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %key.27))))
             (rem (cached #f))
             (ins (cached #t) (n 1)))
           (var-ref %key.27))
         (return (tuple))))
  (circuit %measure.12 (exported #t) (pure #f) (proof #t) ()
    (tunsigned 18446744073709551615)
    (seq (assert
           (== (tboolean)
               (public-ledger %tags.6 read (0) isEmpty (tboolean)
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
               (public-ledger %scores.5 read (1) isEmpty (tboolean)
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
           (public-ledger %tags.6 read (0) size (tunsigned 18446744073709551615)
             (instructions
               (dup (n 0))
               (idx (cached #f) (pushPath #f) (path ((align 0 1))))
               (size)
               (popeq (cached #t) (result (void))))))))
  (circuit %cycle_queue.13 (exported #t) (pure #f) (proof #t)
    ((%v.22 (tunsigned 18446744073709551615)))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tunsigned 18446744073709551615)))
    (seq (public-ledger %queue.7 update (2) pushFront (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
             (dup (n 0))
             (idx (cached #f) (pushPath #f) (path ((align 2 1))))
             (addi (immediate 1))
             (push
               (storage #t)
               (value
                 (state-value
                   array
                   (state-value cell (var-ref %v.22))
                   (state-value null)
                   (state-value null))))
             (swap (n 0))
             (push (storage #f) (value (state-value cell (align 2 1))))
             (swap (n 0)) (ins (cached #t) (n 1)) (swap (n 0))
             (push (storage #f) (value (state-value cell (align 1 1))))
             (swap (n 0)) (ins (cached #t) (n 2)))
           (var-ref %v.22))
         (let* (((%front.21
                   (tstruct
                     Maybe
                     (is_some (tboolean))
                     (value (tunsigned 18446744073709551615)))) (public-ledger %queue.7
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
           (seq (public-ledger %queue.7 remove (2) popFront (ttuple)
                  (instructions
                    (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                    (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                    (ins (cached #t) (n 1))))
                (return (var-ref %front.21))))))
  (circuit %push_queue.10 (exported #t) (pure #f) (proof #t)
    ((%v.17 (tunsigned 18446744073709551615))) (ttuple)
    (seq (public-ledger %queue.7 update (2) pushFront (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
             (dup (n 0))
             (idx (cached #f) (pushPath #f) (path ((align 2 1))))
             (addi (immediate 1))
             (push
               (storage #t)
               (value
                 (state-value
                   array
                   (state-value cell (var-ref %v.17))
                   (state-value null)
                   (state-value null))))
             (swap (n 0))
             (push (storage #f) (value (state-value cell (align 2 1))))
             (swap (n 0)) (ins (cached #t) (n 1)) (swap (n 0))
             (push (storage #f) (value (state-value cell (align 1 1))))
             (swap (n 0)) (ins (cached #t) (n 2)))
           (var-ref %v.17))
         (return (tuple))))
  (circuit %wind_back.4 (exported #t) (pure #f) (proof #t)
    ((%n.20 (tunsigned 65535))
      (%threshold.18 (tunsigned 18446744073709551615)))
    (tboolean)
    (seq (let* (((%tmp.19 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 4)
                                               '4)))
           (public-ledger %rounds.8 update (3) increment (ttuple)
             (instructions
               (idx (cached #f) (pushPath #t) (path ((align 3 1))))
               (addi (immediate (value->int (var-ref %tmp.19))))
               (ins (cached #t) (n 1)))
             (var-ref %tmp.19)))
         (public-ledger %rounds.8 update (3) decrement (ttuple)
           (instructions
             (idx (cached #f) (pushPath #t) (path ((align 3 1))))
             (subi (immediate (value->int (var-ref %n.20))))
             (ins (cached #t) (n 1)))
           (var-ref %n.20))
         (return
           (public-ledger %rounds.8 read (3) lessThan (tboolean)
             (instructions (dup (n 0))
               (idx (cached #f) (pushPath #f) (path ((align 3 1))))
               (push
                 (storage #f)
                 (value (state-value cell (var-ref %threshold.18))))
               (lt) (popeq (cached #t) (result (void))))
             (var-ref %threshold.18)))))
  (circuit %clear_all.16 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (public-ledger %tags.6 remove (0) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push (storage #t) (value (state-value map)))
             (ins (cached #f) (n 1))))
         (public-ledger %scores.5 remove (1) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 1 1))))
             (push (storage #t) (value (state-value map)))
             (ins (cached #f) (n 1))))
         (public-ledger %queue.7 remove (2) resetToDefault (ttuple)
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
         (return (tuple))))
  (circuit %head_point.11 (exported #t) (pure #f) (proof #t) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tpoint (curve-jubjub))))
    (return
      (public-ledger %points.9 read (4) head
        (tstruct
          Maybe
          (is_some (tboolean))
          (value (tpoint (curve-jubjub))))
        (instructions (dup (n 0))
          (idx (cached #f) (pushPath #f) (path ((align 4 1))))
          (idx (cached #f) (pushPath #f) (path ((align 0 1))))
          (dup (n 0)) (type)
          (push (storage #f) (value (state-value cell (align 1 1))))
          (eq) (branch (skip 4))
          (push (storage #f) (value (state-value cell (align 1 1))))
          (swap (n 0))
          (concat
            (cached #f)
            (n (+ 2 (max-sizeof (tpoint (curve-jubjub))))))
          (jmp (skip 2)) (pop)
          (push
            (storage #f)
            (value
              (state-value
                cell
                (aligned-concat
                  (align 0 1)
                  (null (tpoint (curve-jubjub)))))))
          (popeq (cached #t) (result (void))))))))
