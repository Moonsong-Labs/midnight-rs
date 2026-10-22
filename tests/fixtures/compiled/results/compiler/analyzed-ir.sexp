(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (close . %close.4)
    (pair . %pair.5)
    (phase_cell . %phase_cell.2)
    (total . %total.3))
  (contract-types)
  (kernel-declaration (%kernel.8 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%total.3
        (0)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%phase_cell.2
        (1)
        (exported #t)
        (__compact_Cell (tenum Phase open closed))))
    (constructor () (tuple)))
  (export-typedef Phase () (tenum Phase open closed))
  (circuit %pair.5 (exported #t) (pure #f) (proof #t)
    ((%a.6 (tfield (field-native)))
      (%b.7 (tfield (field-native))))
    (ttuple
      (tfield (field-native))
      (tvector 2 (tfield (field-native))))
    (seq (public-ledger %total.3 write (0) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value (state-value cell (var-ref %a.6))))
             (ins (cached #f) (n 1)))
           (var-ref %a.6))
         (return
           (tuple
             (single
               (+ (tfield (field-native)) (var-ref %a.6) (var-ref %b.7)))
             (single
               (tuple (single (var-ref %a.6)) (single (var-ref %b.7))))))))
  (circuit %close.4 (exported #t) (pure #f) (proof #t) ()
    (tenum Phase open closed)
    (seq (public-ledger %phase_cell.2 write (1) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 1 1))))
             (push
               (storage #t)
               (value
                 (state-value
                   cell
                   (enum-ref (tenum Phase open closed) closed))))
             (ins (cached #f) (n 1)))
           (enum-ref (tenum Phase open closed) closed))
         (return (enum-ref (tenum Phase open closed) closed)))))
