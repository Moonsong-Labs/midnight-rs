(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (keccak_struct . %keccak_struct.0)
    (tag_cell . %tag_cell.1))
  (contract-types)
  (kernel-declaration (%kernel.5 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%tag_cell.1
        (0)
        (exported #t)
        (__compact_Cell (tbytes 32))))
    (constructor () (tuple)))
  (export-typedef
    Tagged
    ()
    (tstruct
      Tagged
      (tag (tbytes 32))
      (count (tunsigned 18446744073709551615))))
  (native %keccak256.2 (entry "__compactRuntime.keccak256" circuit)
    (type-arguments
      (tstruct
        Tagged
        (tag (tbytes 32))
        (count (tunsigned 18446744073709551615))))
    ((%value.6
       (tstruct
         Tagged
         (tag (tbytes 32))
         (count (tunsigned 18446744073709551615)))))
    (tbytes 32))
  (circuit %keccak_struct.0 (exported #t) (pure #f) (proof #t)
    ((%t.3
       (tstruct
         Tagged
         (tag (tbytes 32))
         (count (tunsigned 18446744073709551615)))))
    (tbytes 32)
    (let* (((%h.4 (tbytes 32)) (call
                                 %keccak256.2
                                 (var-ref %t.3))))
      (seq (public-ledger %tag_cell.1 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.4))))
               (ins (cached #f) (n 1)))
             (var-ref %h.4))
           (return (var-ref %h.4))))))
