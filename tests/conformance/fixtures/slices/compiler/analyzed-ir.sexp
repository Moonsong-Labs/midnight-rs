(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (byte_pair . %byte_pair.9) (index_bytes . %index_bytes.10)
    (slice_bytes_const . %slice_bytes_const.7)
    (slice_bytes_dynamic . %slice_bytes_dynamic.8)
    (slice_then_index . %slice_then_index.5)
    (slice_tuple_const . %slice_tuple_const.6)
    (slice_vector_const . %slice_vector_const.3)
    (slice_vector_dynamic . %slice_vector_dynamic.4)
    (tail_bytes . %tail_bytes.1)
    (tuple_digest . %tuple_digest.2)
    (vector_digest . %vector_digest.0))
  (contract-types)
  (kernel-declaration (%kernel.36 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%byte_pair.9
        (0)
        (exported #t)
        (__compact_Cell (tunsigned 65535)))
      (%vector_digest.0
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%tuple_digest.2
        (2)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%tail_bytes.1
        (3)
        (exported #t)
        (__compact_Cell (tbytes 4))))
    (constructor () (tuple)))
  (circuit %pack3.21 (exported #f) (pure #t) (proof #f)
    ((%v.33 (tvector 3 (tfield (field-native)))))
    (tfield (field-native))
    (return
      (+ (tfield (field-native))
         (+ (tfield (field-native))
            (* (tfield (field-native))
               (tuple-ref (var-ref %v.33) 0)
               (safe-cast
                 (tfield (field-native))
                 (tunsigned 1000000)
                 '1000000))
            (* (tfield (field-native))
               (tuple-ref (var-ref %v.33) 1)
               (safe-cast (tfield (field-native)) (tunsigned 1000) '1000)))
         (tuple-ref (var-ref %v.33) 2))))
  (circuit %index_bytes.10 (exported #t) (pure #f) (proof #t)
    ((%b.34 (tbytes 8))) (tunsigned 65535)
    (let* (((%packed.35 (tunsigned 65535)) (+ (tunsigned 65535)
                                              (safe-cast
                                                (tunsigned 65535)
                                                (tunsigned 65280)
                                                (* (tunsigned 65280)
                                                   (safe-cast
                                                     (tunsigned 65280)
                                                     (tunsigned 255)
                                                     (bytes-ref
                                                       (tbytes 8)
                                                       (var-ref %b.34)
                                                       '2))
                                                   (safe-cast
                                                     (tunsigned 65280)
                                                     (tunsigned 256)
                                                     '256)))
                                              (safe-cast
                                                (tunsigned 65535)
                                                (tunsigned 255)
                                                (bytes-ref
                                                  (tbytes 8)
                                                  (var-ref %b.34)
                                                  '5)))))
      (seq (public-ledger %byte_pair.9 (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %packed.35))))
               (ins (cached #f) (n 1)))
             (var-ref %packed.35))
           (return (var-ref %packed.35)))))
  (circuit %slice_bytes_const.7 (exported #t) (pure #f) (proof #t)
    ((%b.28 (tbytes 8))) (tbytes 4)
    (let* (((%tail.29 (tbytes 4)) (bytes-slice
                                    (tbytes 8)
                                    (var-ref %b.28)
                                    '3
                                    4)))
      (seq (public-ledger %tail_bytes.1 (3) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 3 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tail.29))))
               (ins (cached #f) (n 1)))
             (var-ref %tail.29))
           (return (var-ref %tail.29)))))
  (circuit %slice_bytes_dynamic.8 (exported #t) (pure #f) (proof #t)
    ((%b.30 (tbytes 8))) (tbytes 4)
    (let* (((%start.31 (tunsigned 1)) '1))
      (let* (((%tail.32 (tbytes 4)) (bytes-slice
                                      (tbytes 8)
                                      (var-ref %b.30)
                                      (var-ref %start.31)
                                      4)))
        (seq (public-ledger %tail_bytes.1 (3) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 3 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %tail.32))))
                 (ins (cached #f) (n 1)))
               (var-ref %tail.32))
             (return (var-ref %tail.32))))))
  (circuit %slice_then_index.5 (exported #t) (pure #f) (proof #t)
    ((%b.22 (tbytes 8))) (tunsigned 65535)
    (let* (((%tail.23 (tbytes 4)) (bytes-slice
                                    (tbytes 8)
                                    (var-ref %b.22)
                                    '3
                                    4)))
      (let* (((%packed.24 (tunsigned 65535)) (+ (tunsigned 65535)
                                                (safe-cast
                                                  (tunsigned 65535)
                                                  (tunsigned 65280)
                                                  (* (tunsigned 65280)
                                                     (safe-cast
                                                       (tunsigned 65280)
                                                       (tunsigned 255)
                                                       (bytes-ref
                                                         (tbytes 4)
                                                         (var-ref %tail.23)
                                                         '0))
                                                     (safe-cast
                                                       (tunsigned 65280)
                                                       (tunsigned 256)
                                                       '256)))
                                                (safe-cast
                                                  (tunsigned 65535)
                                                  (tunsigned 255)
                                                  (bytes-ref
                                                    (tbytes 4)
                                                    (var-ref %tail.23)
                                                    '3)))))
        (seq (public-ledger %byte_pair.9 (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %packed.24))))
                 (ins (cached #f) (n 1)))
               (var-ref %packed.24))
             (return (var-ref %packed.24))))))
  (circuit %slice_vector_const.3 (exported #t) (pure #f) (proof #t)
    ((%xs.25 (tvector 6 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%mid.26 (tvector 3 (tfield (field-native)))) (tuple-slice
                                                            (tvector
                                                              6
                                                              (tfield
                                                                (field-native)))
                                                            (var-ref
                                                              %xs.25)
                                                            2
                                                            3)))
      (let* (((%packed.27 (tfield (field-native))) (call
                                                     %pack3.21
                                                     (var-ref %mid.26))))
        (seq (public-ledger %vector_digest.0 (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %packed.27))))
                 (ins (cached #f) (n 1)))
               (var-ref %packed.27))
             (return (var-ref %packed.27))))))
  (circuit %slice_tuple_const.6 (exported #t) (pure #f) (proof #t)
    ((%a.12 (tunsigned 255))
      (%b.13 (tunsigned 65535))
      (%c.11 (tfield (field-native))))
    (tfield (field-native))
    (let* (((%row.14
              (ttuple
                (tunsigned 255)
                (tunsigned 65535)
                (tfield (field-native))
                (tfield (field-native)))) (tuple
                                            (single (var-ref %a.12))
                                            (single (var-ref %b.13))
                                            (single (var-ref %c.11))
                                            (single
                                              (safe-cast
                                                (tfield (field-native))
                                                (tunsigned 7)
                                                '7)))))
      (let* (((%mid.15
                (ttuple (tunsigned 65535) (tfield (field-native)))) (tuple-slice
                                                                      (ttuple
                                                                        (tunsigned
                                                                          255)
                                                                        (tunsigned
                                                                          65535)
                                                                        (tfield
                                                                          (field-native))
                                                                        (tfield
                                                                          (field-native)))
                                                                      (var-ref
                                                                        %row.14)
                                                                      1
                                                                      2)))
        (let* (((%packed.16 (tfield (field-native))) (+ (tfield
                                                          (field-native))
                                                        (* (tfield
                                                             (field-native))
                                                           (safe-cast
                                                             (tfield
                                                               (field-native))
                                                             (tunsigned
                                                               65535)
                                                             (tuple-ref
                                                               (var-ref
                                                                 %mid.15)
                                                               0))
                                                           (safe-cast
                                                             (tfield
                                                               (field-native))
                                                             (tunsigned
                                                               1000)
                                                             '1000))
                                                        (tuple-ref
                                                          (var-ref %mid.15)
                                                          1))))
          (seq (public-ledger %tuple_digest.2 (2) write (ttuple)
                 (instructions
                   (push
                     (storage #f)
                     (value (state-value cell (align 2 1))))
                   (push
                     (storage #t)
                     (value (state-value cell (var-ref %packed.16))))
                   (ins (cached #f) (n 1)))
                 (var-ref %packed.16))
               (return (var-ref %packed.16)))))))
  (circuit %slice_vector_dynamic.4 (exported #t) (pure #f) (proof #t)
    ((%xs.17 (tvector 6 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%start.18 (tunsigned 1)) '1))
      (let* (((%mid.19 (tvector 3 (tfield (field-native)))) (vector-slice
                                                              (tvector
                                                                6
                                                                (tfield
                                                                  (field-native)))
                                                              (var-ref
                                                                %xs.17)
                                                              (var-ref
                                                                %start.18)
                                                              3)))
        (let* (((%packed.20 (tfield (field-native))) (call
                                                       %pack3.21
                                                       (var-ref %mid.19))))
          (seq (public-ledger %vector_digest.0 (1) write (ttuple)
                 (instructions
                   (push
                     (storage #f)
                     (value (state-value cell (align 1 1))))
                   (push
                     (storage #t)
                     (value (state-value cell (var-ref %packed.20))))
                   (ins (cached #f) (n 1)))
                 (var-ref %packed.20))
               (return (var-ref %packed.20))))))))
