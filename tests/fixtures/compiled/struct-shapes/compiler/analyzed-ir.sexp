(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (amounts . %amounts.5) (colors . %colors.6)
    (counts . %counts.3) (paint . %paint.4) (pairs . %pairs.1)
    (second_color . %second_color.2))
  (contract-types)
  (kernel-declaration (%kernel.11 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%amounts.5
        (0)
        (exported #t)
        (__compact_Cell
          (tstruct
            Pair
            (first (talias #f Amount (tunsigned 18446744073709551615)))
            (second
              (talias #f Amount (tunsigned 18446744073709551615))))))
      (%counts.3
        (1)
        (exported #t)
        (__compact_Cell
          (tstruct
            Pair
            (first (tunsigned 18446744073709551615))
            (second (tunsigned 18446744073709551615)))))
      (%colors.6
        (2)
        (exported #t)
        (__compact_Cell
          (tstruct
            Maybe
            (is_some (tboolean))
            (value (tvector 2 (tenum Color red green))))))
      (%pairs.1
        (3)
        (exported #t)
        (MerkleTree
          4
          (tstruct
            Pair
            (first (tfield (field-native)))
            (second (tfield (field-native)))))))
    (constructor () (tuple)))
  (export-typedef Color () (tenum Color red green))
  (export-typedef
    Pair
    (T)
    (tstruct Pair (first T) (second T)))
  (export-typedef
    Amount
    ()
    (talias #f Amount (tunsigned 18446744073709551615)))
  (circuit %some.8 (exported #f) (pure #t) (proof #f)
    ((%value.10 (tvector 2 (tenum Color red green))))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tvector 2 (tenum Color red green))))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (tvector 2 (tenum Color red green))))
           '#t
           (var-ref %value.10))))
  (circuit %second_color.2 (exported #t) (pure #f) (proof #t)
    ((%c.7
       (tstruct
         Maybe
         (is_some (tboolean))
         (value (tvector 2 (tenum Color red green))))))
    (tenum Color red green)
    (seq (assert (elt-ref (var-ref %c.7) is_some 0) "no colors")
         (public-ledger %colors.6 write (2) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 2 1))))
             (push
               (storage #t)
               (value (state-value cell (var-ref %c.7))))
             (ins (cached #f) (n 1)))
           (var-ref %c.7))
         (return (tuple-ref (elt-ref (var-ref %c.7) value 1) 1))))
  (circuit %paint.4 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (let* (((%tmp.9
                   (tstruct
                     Maybe
                     (is_some (tboolean))
                     (value (tvector 2 (tenum Color red green))))) (call
                                                                     %some.8
                                                                     (tuple
                                                                       (single
                                                                         (enum-ref
                                                                           (tenum
                                                                             Color
                                                                             red
                                                                             green)
                                                                           green))
                                                                       (single
                                                                         (enum-ref
                                                                           (tenum
                                                                             Color
                                                                             red
                                                                             green)
                                                                           red))))))
           (public-ledger %colors.6 write (2) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 2 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.9))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.9)))
         (return (tuple)))))
