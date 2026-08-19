(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (cell . %cell.11)
    (load . %load.12)
    (store . %store.10))
  (contract-types)
  (kernel-declaration (%kernel.20 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%cell.11 (0) (exported #t) (__compact_Cell (tbytes 8))))
    (constructor () (tuple)))
  (export-typedef
    P
    ()
    (tstruct P (a (tunsigned 65535)) (b (tboolean))))
  (circuit %serialize.15 (exported #f) (pure #t) (proof #f)
    ((%value.19
       (tstruct P (a (tunsigned 65535)) (b (tboolean)))))
    (tbytes 8)
    (return
      (vector->bytes
        8
        (vector
          (spread
            2
            (bytes->vector
              2
              (field->bytes
                2
                (field-native)
                (safe-cast
                  (tfield (field-native))
                  (tunsigned 65535)
                  (elt-ref (var-ref %value.19) a 0)))))
          (single
            (if (elt-ref (var-ref %value.19) b 1)
                (safe-cast (tunsigned 255) (tunsigned 1) '1)
                (safe-cast (tunsigned 255) (tunsigned 0) '0)))
          (spread 5 (bytes->vector 5 '#vu8(0 0 0 0 0)))))))
  (circuit %deserialize.13 (exported #f) (pure #t) (proof #f)
    ((%value.14 (tbytes 8)))
    (tstruct P (a (tunsigned 65535)) (b (tboolean)))
    (return
      (new (tstruct P (a (tunsigned 65535)) (b (tboolean)))
           (cast-from-bytes
             (tunsigned 65535)
             2
             (bytes-slice (tbytes 8) (var-ref %value.14) '0 2))
           (== (tunsigned 255)
               (bytes-ref (tbytes 8) (var-ref %value.14) '2)
               (safe-cast (tunsigned 255) (tunsigned 1) '1)))))
  (circuit %store.10 (exported #t) (pure #f) (proof #t)
    ((%x.16 (tunsigned 65535)) (%f.17 (tboolean))) (ttuple)
    (seq (let* (((%tmp.18 (tbytes 8)) (call
                                        %serialize.15
                                        (new (tstruct
                                               P
                                               (a (tunsigned 65535))
                                               (b (tboolean)))
                                             (var-ref %x.16)
                                             (var-ref %f.17)))))
           (public-ledger %cell.11 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.18))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.18)))
         (return (tuple))))
  (circuit %load.12 (exported #t) (pure #f) (proof #t) ()
    (tstruct P (a (tunsigned 65535)) (b (tboolean)))
    (return
      (call
        %deserialize.13
        (public-ledger %cell.11 read (0) read (tbytes 8)
          (instructions
            (dup (n 0))
            (idx (cached #f) (pushPath #f) (path ((align 0 1))))
            (popeq (cached #f) (result (void)))))))))
