(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (fields . %fields.0) (tags . %tags.1))
  (contract-types)
  (kernel-declaration (%kernel.2 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%fields.0
        (0)
        (exported #t)
        (__compact_Cell
          (tstruct
            Pair
            (first (tfield (field-native)))
            (second (tfield (field-native))))))
      (%tags.1
        (1)
        (exported #t)
        (__compact_Cell
          (tstruct Pair (first (tbytes 32)) (second (tbytes 32))))))
    (constructor () (tuple)))
  (export-typedef
    Pair
    (T)
    (tstruct Pair (first T) (second T))))
