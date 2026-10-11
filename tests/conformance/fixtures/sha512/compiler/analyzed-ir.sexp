(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (digest_cell . %digest_cell.0)
    (sha512_struct . %sha512_struct.1))
  (contract-types)
  (kernel-declaration (%kernel.5 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%digest_cell.0
        (0)
        (exported #t)
        (__compact_Cell (tbytes 64))))
    (constructor () (tuple)))
  (export-typedef
    Tagged
    ()
    (tstruct
      Tagged
      (tag (tbytes 32))
      (count (tunsigned 18446744073709551615))))
  (native %sha512.2 (entry "__compactRuntime.sha512" circuit)
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
    (tbytes 64))
  (circuit %sha512_struct.1 (exported #t) (pure #f) (proof #t)
    ((%t.3
       (tstruct
         Tagged
         (tag (tbytes 32))
         (count (tunsigned 18446744073709551615)))))
    (tbytes 64)
    (let* (((%h.4 (tbytes 64)) (call %sha512.2 (var-ref %t.3))))
      (seq (public-ledger %digest_cell.0 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.4))))
               (ins (cached #f) (n 1)))
             (var-ref %h.4))
           (return (var-ref %h.4))))))
