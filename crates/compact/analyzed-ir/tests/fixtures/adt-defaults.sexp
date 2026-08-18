(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (items . %items.2)
    (peek . %peek.3)
    (table . %table.0)
    (touch . %touch.1))
  (contract-types)
  (kernel-declaration (%kernel.5 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%items.2
        (0)
        (exported #t)
        (List (tunsigned 18446744073709551615)))
      (%table.0
        (1)
        (exported #t)
        (Map (tunsigned 18446744073709551615) (Counter))))
    (constructor () (tuple)))
  (circuit %peek.3 (exported #t) (pure #f) (proof #t) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tunsigned 18446744073709551615)))
    (return
      (public-ledger %items.2 read (0) head
        (tstruct
          Maybe
          (is_some (tboolean))
          (value (tunsigned 18446744073709551615)))
        (instructions (dup (n 0))
          (idx (cached #f) (pushPath #f) (path ((align 0 1))))
          (idx (cached #f) (pushPath #f) (path ((align 0 1))))
          (dup (n 0)) (type)
          (push (storage #f) (value (state-value cell (align 1 1))))
          (eq) (branch (skip 4))
          (push (storage #f) (value (state-value cell (align 1 1))))
          (swap (n 0))
          (concat
            (cached #f)
            (n (+ 2 (max-sizeof (tunsigned 18446744073709551615)))))
          (jmp (skip 2)) (pop)
          (push
            (storage #f)
            (value
              (state-value
                cell
                (aligned-concat
                  (align 0 1)
                  (null (tunsigned 18446744073709551615))))))
          (popeq (cached #t) (result (void)))))))
  (circuit %touch.1 (exported #t) (pure #f) (proof #t)
    ((%k.4 (tunsigned 18446744073709551615))) (ttuple)
    (seq (public-ledger %table.0 update (1) insertDefault (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %k.4))))
             (push
               (storage #t)
               (value (state-value ADT (null (Counter)) (Counter))))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %k.4))
         (return (tuple)))))
