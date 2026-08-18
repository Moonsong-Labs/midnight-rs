(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (cell . %cell.1)
    (load . %load.2)
    (store . %store.0))
  (contract-types)
  (kernel-declaration (%kernel.10 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%cell.1 (0) (exported #t) (__compact_Cell (tbytes 8))))
    (constructor () (tuple)))
  (export-typedef
    P
    ()
    (tstruct P (a (tunsigned 65535)) (b (tboolean))))
  (circuit %serialize.8 (exported #f) (pure #t) (proof #f)
    ((%value.9
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
                  (elt-ref (var-ref %value.9) a 0)))))
          (single
            (if (elt-ref (var-ref %value.9) b 1)
                (safe-cast (tunsigned 255) (tunsigned 1) '1)
                (safe-cast (tunsigned 255) (tunsigned 0) '0)))
          (spread 5 (bytes->vector 5 '#vu8(0 0 0 0 0)))))))
  (circuit %deserialize.3 (exported #f) (pure #t) (proof #f)
    ((%value.4 (tbytes 8)))
    (tstruct P (a (tunsigned 65535)) (b (tboolean)))
    (return
      (new (tstruct P (a (tunsigned 65535)) (b (tboolean)))
           (cast-from-bytes
             (tunsigned 65535)
             2
             (bytes-slice (tbytes 8) (var-ref %value.4) '0 2))
           (== (tunsigned 255)
               (bytes-ref (tbytes 8) (var-ref %value.4) '2)
               (safe-cast (tunsigned 255) (tunsigned 1) '1)))))
  (circuit %store.0 (exported #t) (pure #f) (proof #t)
    ((%x.5 (tunsigned 65535)) (%f.6 (tboolean))) (ttuple)
    (seq (let* (((%tmp.7 (tbytes 8)) (call
                                       %serialize.8
                                       (new (tstruct
                                              P
                                              (a (tunsigned 65535))
                                              (b (tboolean)))
                                            (var-ref %x.5)
                                            (var-ref %f.6)))))
           (public-ledger %cell.1 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.7))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.7)))
         (return (tuple))))
  (circuit %load.2 (exported #t) (pure #f) (proof #t) ()
    (tstruct P (a (tunsigned 65535)) (b (tboolean)))
    (return
      (call
        %deserialize.3
        (public-ledger %cell.1 read (0) read (tbytes 8)
          (instructions
            (dup (n 0))
            (idx (cached #f) (pushPath #f) (path ((align 0 1))))
            (popeq (cached #f) (result (void)))))))))
