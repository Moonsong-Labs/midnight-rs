(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (add_voter . %add_voter.136) (advance . %advance.137)
   (set_topic . %set_topic.134)
   (vote$commit . %vote$commit.135)
   (vote$reveal . %vote$reveal.133))
 (contract-types)
 (kernel-declaration (%kernel.210 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%authority.154
       (0)
       (exported #f)
       (__compact_Cell (tbytes 32)))
     (%state.155
       (1)
       (exported #f)
       (__compact_Cell
         (tenum PublicState setup commit reveal final)))
     (%topic.153
       (2)
       (exported #f)
       (__compact_Cell
         (tstruct
           Maybe
           (is_some (tboolean))
           (value (topaque "string")))))
     (%tally_yes.190 (3) (exported #f) (Counter))
     (%tally_no.192 (4) (exported #f) (Counter))
     (%committed_votes.174
       (5)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%eligible_voters.159
       (6)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%committed.175 (7) (exported #f) (Set (tbytes 32)))
     (%revealed.187 (8) (exported #f) (Set (tbytes 32))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.177 (exported #f) (pure #t) (proof #f)
   ((%path.204
      (tstruct
        MerkleTreePath
        (leaf (tbytes 32))
        (path
          (tvector
            10
            (tstruct
              MerkleTreePathEntry
              (sibling
                (tstruct MerkleTreeDigest (field (tfield (field-native)))))
              (goes_left (tboolean))))))))
   (tstruct MerkleTreeDigest (field (tfield (field-native))))
   (return
     (new (tstruct
            MerkleTreeDigest
            (field (tfield (field-native))))
          (fold
            10
            (fref %merkleTreePathEntryRoot.205)
            ((call
               %degradeToTransient.198
               (call
                 %persistentHash.202
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tbytes 32)))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.204) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.204) path 1)
              (tvector
                10
                (tstruct
                  MerkleTreePathEntry
                  (sibling
                    (tstruct
                      MerkleTreeDigest
                      (field (tfield (field-native)))))
                  (goes_left (tboolean))))
              (tstruct
                MerkleTreePathEntry
                (sibling
                  (tstruct
                    MerkleTreeDigest
                    (field (tfield (field-native)))))
                (goes_left (tboolean))))))))
 (circuit %merkleTreePathEntryRoot.205 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.206 (tfield (field-native)))
     (%entry.207
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.208 (tfield (field-native))) (if (elt-ref
                                                     (var-ref %entry.207)
                                                     goes_left
                                                     1)
                                                   (var-ref
                                                     %recursiveDigest.206)
                                                   (elt-ref
                                                     (elt-ref
                                                       (var-ref %entry.207)
                                                       sibling
                                                       0)
                                                     field
                                                     0))))
     (let* (((%right.209 (tfield (field-native))) (if (elt-ref
                                                        (var-ref
                                                          %entry.207)
                                                        goes_left
                                                        1)
                                                      (elt-ref
                                                        (elt-ref
                                                          (var-ref
                                                            %entry.207)
                                                          sibling
                                                          0)
                                                        field
                                                        0)
                                                      (var-ref
                                                        %recursiveDigest.206))))
       (return
         (call
           %transientHash.200
           (tuple
             (single (var-ref %left.208))
             (single (var-ref %right.209))))))))
 (native
   %transientHash.200
   (entry "__compactRuntime.transientHash" circuit)
   ((%value.201 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native
   %persistentHash.202
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.203
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tbytes 32)))))
   (tbytes 32))
 (native
   %persistentHash.140
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.197 (tvector 2 (tbytes 32))))
   (tbytes 32))
 (native
   %degradeToTransient.198
   (entry "__compactRuntime.degradeToTransient" circuit)
   ((%x.199 (tbytes 32)))
   (tfield (field-native)))
 (witness %private$secret_key.150 () (tbytes 32))
 (witness
   %private$state.178
   ()
   (tenum PrivateState initial committed revealed))
 (witness %private$state$advance.173 () (ttuple))
 (witness
   %private$vote$record.179
   ((%ballot.196 (tenum PermissibleVotes yes no)))
   (ttuple))
 (witness
   %private$vote.183
   ()
   (tenum PermissibleVotes yes no))
 (witness
   %context$eligible_voters$path_of.160
   ((%pk.195 (tbytes 32)))
   (tstruct
     Maybe
     (is_some (tboolean))
     (value
       (tstruct
         MerkleTreePath
         (leaf (tbytes 32))
         (path
           (tvector
             10
             (tstruct
               MerkleTreePathEntry
               (sibling
                 (tstruct
                   MerkleTreeDigest
                   (field (tfield (field-native)))))
               (goes_left (tboolean)))))))))
 (witness
   %context$committed_votes$path_of.186
   ((%cm.193 (tbytes 32)))
   (tstruct
     Maybe
     (is_some (tboolean))
     (value
       (tstruct
         MerkleTreePath
         (leaf (tbytes 32))
         (path
           (tvector
             10
             (tstruct
               MerkleTreePathEntry
               (sibling
                 (tstruct
                   MerkleTreeDigest
                   (field (tfield (field-native)))))
               (goes_left (tboolean)))))))))
 (circuit %ballot_repr.172 (exported #f) (pure #t) (proof #f)
   ((%ballot.194 (tenum PermissibleVotes yes no))) (tbytes 32)
   (return
     (if (== (tenum PermissibleVotes yes no)
             (var-ref %ballot.194)
             (enum-ref (tenum PermissibleVotes yes no) yes))
         '#vu8(121 101 115 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0 0)
         '#vu8(110 111 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0))))
 (circuit %vote$commit.135 (exported #t) (pure #f) (proof #t)
   ((%ballot.166 (tenum PermissibleVotes yes no))) (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         commit))
                   (== (tenum PrivateState initial committed revealed)
                       (call %private$state.178)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         initial))
                   '#f)
               "In illegal state for committing")
             (call %private$vote$record.179 (var-ref %ballot.166))
             (let* (((%sk.167 (tbytes 32)) (call
                                             %private$secret_key.150)))
               (let* (((%com_nul.168 (tbytes 32)) (call
                                                    %commitment_nullifier.144
                                                    (var-ref %sk.167))))
                 (seq (assert
                        (if (public-ledger %committed.175 (7) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 7 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %com_nul.168))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %com_nul.168))
                            '#f
                            '#t)
                        "Unexpected attempt to double use of nullifier")
                      (let* (((%pk.169 (tbytes 32)) (call
                                                      %public_key.138
                                                      (var-ref %sk.167))))
                        (let* (((%path.170
                                  (tstruct
                                    Maybe
                                    (is_some (tboolean))
                                    (value
                                      (tstruct
                                        MerkleTreePath
                                        (leaf (tbytes 32))
                                        (path
                                          (tvector
                                            10
                                            (tstruct
                                              MerkleTreePathEntry
                                              (sibling
                                                (tstruct
                                                  MerkleTreeDigest
                                                  (field
                                                    (tfield
                                                      (field-native)))))
                                              (goes_left (tboolean))))))))) (call
                                                                              %context$eligible_voters$path_of.160
                                                                              (var-ref
                                                                                %pk.169))))
                          (seq (assert
                                 (if (if (elt-ref
                                           (var-ref %path.170)
                                           is_some
                                           0)
                                         (let* (((%tmp.176
                                                   (tstruct
                                                     MerkleTreeDigest
                                                     (field
                                                       (tfield
                                                         (field-native))))) (call
                                                                              %merkleTreePathRoot.177
                                                                              (elt-ref
                                                                                (var-ref
                                                                                  %path.170)
                                                                                value
                                                                                1))))
                                           (public-ledger %eligible_voters.159 (6)
                                             checkRoot (tboolean)
                                             (instructions (dup (n 0))
                                               (idx (cached #f)
                                                    (pushPath #f)
                                                    (path ((align 6 1))))
                                               (idx (cached #f)
                                                    (pushPath #f)
                                                    (path ((align 0 1))))
                                               (root)
                                               (push
                                                 (storage #f)
                                                 (value
                                                   (state-value
                                                     cell
                                                     (var-ref %tmp.176))))
                                               (eq)
                                               (popeq
                                                 (cached #t)
                                                 (result (void))))
                                             (var-ref %tmp.176)))
                                         '#f)
                                     (== (tbytes 32)
                                         (var-ref %pk.169)
                                         (elt-ref
                                           (elt-ref
                                             (var-ref %path.170)
                                             value
                                             1)
                                           leaf
                                           0))
                                     '#f)
                                 "Attempted to vote without authorization - need to add-voter")
                               (let* (((%cm.171 (tbytes 32)) (call
                                                               %commit_with_sk.141
                                                               (call
                                                                 %ballot_repr.172
                                                                 (var-ref
                                                                   %ballot.166))
                                                               (var-ref
                                                                 %sk.167))))
                                 (seq (public-ledger %committed_votes.174 (5) insert
                                        (ttuple)
                                        (instructions
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 5 1))))
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 0 1))))
                                          (dup (n 2))
                                          (idx (cached #f)
                                               (pushPath #f)
                                               (path ((align 1 1))))
                                          (push
                                            (storage #t)
                                            (value
                                              (state-value
                                                cell
                                                (leaf-hash
                                                  (var-ref %cm.171)))))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1))
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 1 1))))
                                          (addi (immediate 1))
                                          (ins (cached #t) (n 2)))
                                        (var-ref %cm.171))
                                      (public-ledger %committed.175 (7) insert (ttuple)
                                        (instructions
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 7 1))))
                                          (push
                                            (storage #f)
                                            (value
                                              (state-value
                                                cell
                                                (var-ref %com_nul.168))))
                                          (push
                                            (storage #t)
                                            (value (state-value null)))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1)))
                                        (var-ref %com_nul.168))
                                      (call
                                        %private$state$advance.173))))))))))
        (return (tuple))))
 (circuit %vote$reveal.133 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         reveal))
                   (== (tenum PrivateState initial committed revealed)
                       (call %private$state.178)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         committed))
                   '#f)
               "In illegal state for revealing")
             (let* (((%sk.180 (tbytes 32)) (call
                                             %private$secret_key.150)))
               (let* (((%rev_nul.181 (tbytes 32)) (call
                                                    %reveal_nullifier.146
                                                    (var-ref %sk.180))))
                 (seq (assert
                        (if (public-ledger %revealed.187 (8) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 8 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %rev_nul.181))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %rev_nul.181))
                            '#f
                            '#t)
                        "Attempted to double vote")
                      (let* (((%vote.182 (tenum PermissibleVotes yes no)) (call
                                                                            %private$vote.183)))
                        (let* (((%cm.184 (tbytes 32)) (call
                                                        %commit_with_sk.141
                                                        (call
                                                          %ballot_repr.172
                                                          (var-ref
                                                            %vote.182))
                                                        (var-ref
                                                          %sk.180))))
                          (let* (((%path.185
                                    (tstruct
                                      Maybe
                                      (is_some (tboolean))
                                      (value
                                        (tstruct
                                          MerkleTreePath
                                          (leaf (tbytes 32))
                                          (path
                                            (tvector
                                              10
                                              (tstruct
                                                MerkleTreePathEntry
                                                (sibling
                                                  (tstruct
                                                    MerkleTreeDigest
                                                    (field
                                                      (tfield
                                                        (field-native)))))
                                                (goes_left
                                                  (tboolean))))))))) (call
                                                                       %context$committed_votes$path_of.186
                                                                       (var-ref
                                                                         %cm.184))))
                            (seq (assert
                                   (if (if (elt-ref
                                             (var-ref %path.185)
                                             is_some
                                             0)
                                           (let* (((%tmp.188
                                                     (tstruct
                                                       MerkleTreeDigest
                                                       (field
                                                         (tfield
                                                           (field-native))))) (call
                                                                                %merkleTreePathRoot.177
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %path.185)
                                                                                  value
                                                                                  1))))
                                             (public-ledger %committed_votes.174 (5)
                                               checkRoot (tboolean)
                                               (instructions (dup (n 0))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 5 1))))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 0 1))))
                                                 (root)
                                                 (push
                                                   (storage #f)
                                                   (value
                                                     (state-value
                                                       cell
                                                       (var-ref
                                                         %tmp.188))))
                                                 (eq)
                                                 (popeq
                                                   (cached #t)
                                                   (result (void))))
                                               (var-ref %tmp.188)))
                                           '#f)
                                       (== (tbytes 32)
                                           (var-ref %cm.184)
                                           (elt-ref
                                             (elt-ref
                                               (var-ref %path.185)
                                               value
                                               1)
                                             leaf
                                             0))
                                       '#f)
                                   "Attempted to reveal incorrectly")
                                 (if (== (tenum PermissibleVotes yes no)
                                         (var-ref %vote.182)
                                         (enum-ref
                                           (tenum PermissibleVotes yes no)
                                           yes))
                                     (let* (((%tmp.189 (tunsigned 65535)) (safe-cast
                                                                            (tunsigned
                                                                              65535)
                                                                            (tunsigned
                                                                              1)
                                                                            '1)))
                                       (public-ledger %tally_yes.190 (3) increment
                                         (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 3 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.189))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.189)))
                                     (let* (((%tmp.191 (tunsigned 65535)) (safe-cast
                                                                            (tunsigned
                                                                              65535)
                                                                            (tunsigned
                                                                              1)
                                                                            '1)))
                                       (public-ledger %tally_no.192 (4) increment
                                         (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 4 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.191))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.191))))
                                 (public-ledger %revealed.187 (8) insert (ttuple)
                                   (instructions
                                     (idx (cached #f)
                                          (pushPath #t)
                                          (path ((align 8 1))))
                                     (push
                                       (storage #f)
                                       (value
                                         (state-value
                                           cell
                                           (var-ref %rev_nul.181))))
                                     (push
                                       (storage #t)
                                       (value (state-value null)))
                                     (ins (cached #f) (n 1))
                                     (ins (cached #t) (n 1)))
                                   (var-ref %rev_nul.181))
                                 (call %private$state$advance.173)))))))))
        (return (tuple))))
 (circuit %advance.137 (exported #t) (pure #f) (proof #t) () (ttuple)
   (seq (let* (((%sk.161 (tbytes 32)) (call
                                        %private$secret_key.150)))
          (let* (((%apk.162 (tbytes 32)) (call
                                           %public_key.138
                                           (var-ref %sk.161))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.162)
                       (public-ledger %authority.154 (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to advance state without authorization")
                 (assert
                   (elt-ref
                     (public-ledger %topic.153 (2) read
                       (tstruct
                         Maybe
                         (is_some (tboolean))
                         (value (topaque "string")))
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 2 1))))
                         (popeq (cached #f) (result (void)))))
                     is_some
                     0)
                   "Attempted to start election without a topic")
                 (let* (((%tmp.163
                           (tenum PublicState setup commit reveal final)) (call
                                                                            %successor.164
                                                                            (public-ledger
                                                                              %state.155
                                                                              (1)
                                                                              read
                                                                              (tenum
                                                                                PublicState
                                                                                setup
                                                                                commit
                                                                                reveal
                                                                                final)
                                                                              (instructions
                                                                                (dup (n 0))
                                                                                (idx (cached
                                                                                       #f)
                                                                                     (pushPath
                                                                                       #f)
                                                                                     (path
                                                                                       ((align
                                                                                          1
                                                                                          1))))
                                                                                (popeq
                                                                                  (cached
                                                                                    #f)
                                                                                  (result
                                                                                    (void))))))))
                   (public-ledger %state.155 (1) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 1 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.163))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.163))))))
        (return (tuple))))
 (circuit %successor.164 (exported #f) (pure #t) (proof #f)
   ((%state.165 (tenum PublicState setup commit reveal final)))
   (tenum PublicState setup commit reveal final)
   (if (== (tenum PublicState setup commit reveal final)
           (var-ref %state.165)
           (enum-ref
             (tenum PublicState setup commit reveal final)
             setup))
       (return
         (enum-ref
           (tenum PublicState setup commit reveal final)
           commit))
       (if (== (tenum PublicState setup commit reveal final)
               (var-ref %state.165)
               (enum-ref
                 (tenum PublicState setup commit reveal final)
                 commit))
           (return
             (enum-ref
               (tenum PublicState setup commit reveal final)
               reveal))
           (return
             (enum-ref
               (tenum PublicState setup commit reveal final)
               final)))))
 (circuit %set_topic.134 (exported #t) (pure #f) (proof #t)
   ((%t.148 (topaque "string"))) (ttuple)
   (seq (let* (((%sk.149 (tbytes 32)) (call
                                        %private$secret_key.150)))
          (let* (((%apk.151 (tbytes 32)) (call
                                           %public_key.138
                                           (var-ref %sk.149))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.151)
                       (public-ledger %authority.154 (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to set topic without authorization")
                 (assert
                   (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         setup))
                   "Attempted to set topic after setup phase")
                 (let* (((%tmp.152
                           (tstruct
                             Maybe
                             (is_some (tboolean))
                             (value (topaque "string")))) (new (tstruct
                                                                 Maybe
                                                                 (is_some
                                                                   (tboolean))
                                                                 (value
                                                                   (topaque
                                                                     "string")))
                                                               '#t
                                                               (var-ref
                                                                 %t.148))))
                   (public-ledger %topic.153 (2) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 2 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.152))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.152))))))
        (return (tuple))))
 (circuit %add_voter.136 (exported #t) (pure #f) (proof #t)
   ((%pk.156 (tbytes 32))) (ttuple)
   (seq (seq (assert
               (if (elt-ref
                     (call
                       %context$eligible_voters$path_of.160
                       (var-ref %pk.156))
                     is_some
                     0)
                   '#f
                   '#t)
               "Attempted to add a voter twice")
             (let* (((%sk.157 (tbytes 32)) (call
                                             %private$secret_key.150)))
               (let* (((%apk.158 (tbytes 32)) (call
                                                %public_key.138
                                                (var-ref %sk.157))))
                 (seq (assert
                        (== (tbytes 32)
                            (var-ref %apk.158)
                            (public-ledger %authority.154 (0) read (tbytes 32)
                              (instructions
                                (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 0 1))))
                                (popeq (cached #f) (result (void))))))
                        "Attempted to add a voter without authorization")
                      (assert
                        (== (tenum PublicState setup commit reveal final)
                            (public-ledger %state.155 (1) read
                              (tenum PublicState setup commit reveal final)
                              (instructions
                                (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 1 1))))
                                (popeq (cached #f) (result (void)))))
                            (enum-ref
                              (tenum PublicState setup commit reveal final)
                              setup))
                        "Attempted to add a voter after setup phase")
                      (public-ledger %eligible_voters.159 (6) insert (ttuple)
                        (instructions
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 6 1))))
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 0 1))))
                          (dup (n 2))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 1 1))))
                          (push
                            (storage #t)
                            (value
                              (state-value
                                cell
                                (leaf-hash (var-ref %pk.156)))))
                          (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (addi (immediate 1)) (ins (cached #t) (n 2)))
                        (var-ref %pk.156))))))
        (return (tuple))))
 (circuit %commitment_nullifier.144 (exported #f) (pure #t)
   (proof #f) ((%sk.145 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.140
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 99 109 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.145))))))
 (circuit %reveal_nullifier.146 (exported #f) (pure #t) (proof #f)
   ((%sk.147 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.140
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 114 118 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.147))))))
 (circuit %public_key.138 (exported #f) (pure #t) (proof #f)
   ((%sk.139 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.140
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 112 107 58 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.139))))))
 (circuit %commit_with_sk.141 (exported #f) (pure #t) (proof #f)
   ((%ballot.142 (tbytes 32)) (%sk.143 (tbytes 32)))
   (tbytes 32)
   (return
     (call
       %persistentHash.140
       (tuple
         (single (var-ref %ballot.142))
         (single (var-ref %sk.143)))))))
